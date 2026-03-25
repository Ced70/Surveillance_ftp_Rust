use crate::client_ftp::ClientFTP;
use crate::client_piia::ClientPIIA;
use crate::compteur::CompteurPersistant;
use crate::config::AppConfig;
use crate::plc::{self, ResultatPiece};
use crate::serveur_ftp::demarrer_serveur_ftp;
use crate::surveillant::{attendre_fichier_complet, SurveillantImages};
use eframe::egui;
use log::{debug, error, info, warn};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Message de statut affiché dans l'interface
struct Statut {
    texte: String,
    couleur: egui::Color32,
}

/// Resultat de l'analyse PIIA pour affichage
#[derive(Clone)]
#[allow(dead_code)]
struct ResultatPiia {
    piece_ok: bool,
    global_status: String,
    processing_time_ms: u64,
    details: String,
}

/// État partagé entre le thread worker et l'interface
struct EtatPartage {
    statut: Statut,
    image_texture: Option<egui::TextureHandle>,
    /// Données brutes d'une image à charger comme texture (nom, bytes partagés)
    image_a_charger: Option<(String, Arc<[u8]>)>,
    /// Contexte egui pour demander un repaint depuis le worker
    ctx: Option<egui::Context>,
    /// Dernier resultat PIIA
    resultat_piia: Option<ResultatPiia>,
}

pub struct ApplicationGUI {
    config: AppConfig,
    compteur: CompteurPersistant,
    etat: Arc<Mutex<EtatPartage>>,
    _surveillant: Option<SurveillantImages>,
    _serveur_handle: Option<std::thread::JoinHandle<()>>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    arret: Arc<AtomicBool>,
    touche_quitter: egui::Key,
}

/// Convertit une chaîne de config en egui::Key
fn parser_touche(nom: &str) -> egui::Key {
    match nom.to_lowercase().as_str() {
        "escape" | "echap" => egui::Key::Escape,
        "f1" => egui::Key::F1,
        "f2" => egui::Key::F2,
        "f3" => egui::Key::F3,
        "f4" => egui::Key::F4,
        "f5" => egui::Key::F5,
        "f6" => egui::Key::F6,
        "f7" => egui::Key::F7,
        "f8" => egui::Key::F8,
        "f9" => egui::Key::F9,
        "f10" => egui::Key::F10,
        "f11" => egui::Key::F11,
        "f12" => egui::Key::F12,
        "q" => egui::Key::Q,
        _ => {
            warn!("Touche '{}' non reconnue, utilisation de Escape par défaut", nom);
            egui::Key::Escape
        }
    }
}

impl ApplicationGUI {
    pub fn new(config: AppConfig) -> Self {
        let compteur_path = std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("compteur.json");
        let compteur = CompteurPersistant::new(&compteur_path);

        // Créer le répertoire surveillé
        let _ = std::fs::create_dir_all(&config.surveillance.repertoire_surveille);

        // Démarrer le serveur FTP intégré
        let serveur_handle = demarrer_serveur_ftp(
            &config.serveur_ftp,
            &config.surveillance.repertoire_surveille,
        );

        let etat = Arc::new(Mutex::new(EtatPartage {
            statut: Statut {
                texte: "En attente de nouvelles images...".to_string(),
                couleur: egui::Color32::from_rgb(0, 204, 0),
            },
            image_texture: None,
            image_a_charger: None,
            ctx: None,
            resultat_piia: None,
        }));

        // Signal d'arrêt pour les threads
        let arret = Arc::new(AtomicBool::new(false));

        // Canal pour le worker d'upload sérialisé
        let (upload_sender, upload_receiver) = crossbeam_channel::unbounded::<PathBuf>();

        // Démarrer la surveillance watchdog
        let surveillant = match SurveillantImages::new(
            &config.surveillance.repertoire_surveille,
            config.surveillance.extensions.clone(),
            upload_sender.clone(),
        ) {
            Ok(s) => Some(s),
            Err(e) => {
                error!("Impossible de démarrer le watcher : {}", e);
                None
            }
        };

        // Initialiser le client PIIA si actif
        let client_piia = if config.piia.actif {
            info!("PIIA actif : {} (timeout={}ms)", config.piia.url, config.piia.timeout_ms);
            Some(ClientPIIA::new(config.piia.clone()))
        } else {
            info!("PIIA desactive");
            None
        };

        // Initialiser le driver PLC/automate
        let mut plc_driver = plc::creer_driver(&config.automate);
        if let Some(ref mut driver) = plc_driver {
            info!("Automate {} : connexion...", driver.name());
            if let Err(e) = driver.connect() {
                error!("Impossible de connecter l'automate {} : {}", driver.name(), e);
            }
        } else {
            info!("Automate desactive");
        };

        let mut client_ftp = ClientFTP::new(config.ftp_client.clone(), compteur.clone());
        let worker_etat = etat.clone();
        let worker_arret = arret.clone();

        let timeout_fichier = Duration::from_secs(config.interface.timeout_fichier_secs);
        let retries = config.interface.retries_upload;

        let touche_quitter = parser_touche(&config.interface.touche_quitter);

        let worker_handle = std::thread::spawn(move || {
            while !worker_arret.load(Ordering::Relaxed) {
                // Attendre un message avec timeout
                let chemin = match upload_receiver.recv_timeout(Duration::from_secs(1)) {
                    Ok(c) => c,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                let nom = chemin
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                debug!("Nouvelle image detectee : {} ({})", nom, chemin.display());

                // Attendre que le fichier soit complètement écrit
                if !attendre_fichier_complet(&chemin, timeout_fichier) {
                    warn!("Fichier incomplet après timeout : {}", chemin.display());
                    // Supprimer le fichier incomplet
                    let _ = std::fs::remove_file(&chemin);
                    let mut etat = worker_etat.lock().unwrap();
                    etat.statut = Statut {
                        texte: format!("Fichier incomplet : {}", nom),
                        couleur: egui::Color32::from_rgb(255, 68, 68),
                    };
                    if let Some(ctx) = &etat.ctx {
                        ctx.request_repaint();
                    }
                    continue;
                }

                // Lire le fichier une seule fois
                let image_data: Arc<[u8]> = match std::fs::read(&chemin) {
                    Ok(data) => data.into(),
                    Err(e) => {
                        error!("Impossible de lire {} : {}", chemin.display(), e);
                        continue;
                    }
                };

                // --- Analyse PIIA (prioritaire) ---
                if let Some(ref piia) = client_piia {
                    {
                        let mut etat = worker_etat.lock().unwrap();
                        etat.statut = Statut {
                            texte: format!("Analyse PIIA en cours : {}", nom),
                            couleur: egui::Color32::from_rgb(255, 170, 0),
                        };
                        // Afficher l'image brute en attendant le resultat
                        etat.image_a_charger = Some((nom.clone(), Arc::clone(&image_data)));
                        etat.resultat_piia = None;
                        if let Some(ctx) = &etat.ctx {
                            ctx.request_repaint();
                        }
                    }

                    debug!("Envoi image a PIIA : {} ({} octets)", nom, image_data.len());

                    match piia.analyser(&nom, &image_data) {
                        Some(resultat) => {
                            let piece_ok = resultat.piece_ok;
                            let annotated: Arc<[u8]> = resultat.image_annotee.clone().into();

                            debug!(
                                "PIIA resultat pour {} : status={}, piece_ok={}, temps={}ms",
                                nom, resultat.global_status, piece_ok, resultat.processing_time_ms
                            );

                            // Afficher l'image annotee de PIIA
                            {
                                let mut etat = worker_etat.lock().unwrap();
                                etat.image_a_charger =
                                    Some((format!("{}_piia", nom), annotated));
                                etat.resultat_piia = Some(ResultatPiia {
                                    piece_ok: resultat.piece_ok,
                                    global_status: resultat.global_status.clone(),
                                    processing_time_ms: resultat.processing_time_ms,
                                    details: resultat.details,
                                });
                                etat.statut = if piece_ok {
                                    Statut {
                                        texte: format!(
                                            "OK - {} ({}ms)",
                                            nom, resultat.processing_time_ms
                                        ),
                                        couleur: egui::Color32::from_rgb(0, 204, 0),
                                    }
                                } else {
                                    Statut {
                                        texte: format!(
                                            "NOK [{}] - {} ({}ms)",
                                            resultat.global_status, nom, resultat.processing_time_ms
                                        ),
                                        couleur: egui::Color32::from_rgb(255, 68, 68),
                                    }
                                };
                                if let Some(ctx) = &etat.ctx {
                                    ctx.request_repaint();
                                }
                            }

                            // Communiquer le resultat a l'automate/PLC
                            if let Some(ref mut driver) = plc_driver {
                                let plc_res = if piece_ok { ResultatPiece::Ok } else { ResultatPiece::Nok };
                                if let Err(e) = driver.write_result(plc_res) {
                                    error!("Erreur ecriture PLC {} : {}", driver.name(), e);
                                }
                            }
                        }
                        None => {
                            // Timeout ou erreur PIIA (non lance, non joignable, etc.)
                            warn!("PIIA indisponible pour {} - analyse ignoree", nom);
                            {
                                let mut etat = worker_etat.lock().unwrap();
                                etat.statut = Statut {
                                    texte: format!("PIIA indisponible - {} (analyse ignoree)", nom),
                                    couleur: egui::Color32::from_rgb(255, 170, 0),
                                };
                                etat.resultat_piia = None;
                                if let Some(ctx) = &etat.ctx {
                                    ctx.request_repaint();
                                }
                            }

                            // Informer l'automate du timeout (= NOK par securite)
                            if let Some(ref mut driver) = plc_driver {
                                if let Err(e) = driver.write_result(ResultatPiece::Timeout) {
                                    error!("Erreur ecriture PLC timeout {} : {}", driver.name(), e);
                                }
                            }

                            // On continue quand meme avec l'envoi FTP pour archivage
                        }
                    }
                } else {
                    // Pas de PIIA - afficher l'image brute
                    let mut etat = worker_etat.lock().unwrap();
                    etat.image_a_charger = Some((nom.clone(), Arc::clone(&image_data)));
                    etat.statut = Statut {
                        texte: format!("Envoi en cours : {}", nom),
                        couleur: egui::Color32::from_rgb(255, 170, 0),
                    };
                    etat.resultat_piia = None;
                    if let Some(ctx) = &etat.ctx {
                        ctx.request_repaint();
                    }
                }

                // Supprimer l'image source immediatement (donnees deja en memoire)
                // Evite de surcharger le disque local
                if chemin.exists() {
                    if let Err(e) = std::fs::remove_file(&chemin) {
                        warn!("Impossible de supprimer {} : {}", chemin.display(), e);
                    } else {
                        info!("Image source supprimee : {}", chemin.display());
                    }
                }

                // --- Envoi FTP (apres analyse PIIA) ---
                let succes = client_ftp.envoyer_avec_retry(&nom, &image_data, retries);

                {
                    let mut etat = worker_etat.lock().unwrap();
                    if succes {
                        // Garder le statut PIIA s'il existe, sinon afficher envoi OK
                        if etat.resultat_piia.is_none() {
                            etat.statut = Statut {
                                texte: format!("Envoyé : {}", nom),
                                couleur: egui::Color32::from_rgb(0, 204, 0),
                            };
                        }
                    } else {
                        etat.statut = Statut {
                            texte: format!("Échec envoi FTP ({} retries) : {}", retries, nom),
                            couleur: egui::Color32::from_rgb(255, 68, 68),
                        };
                    }
                    if let Some(ctx) = &etat.ctx {
                        ctx.request_repaint();
                    }
                }
            }
            // Deconnecter le PLC proprement
            if let Some(ref mut driver) = plc_driver {
                info!("Deconnexion automate {}...", driver.name());
                if let Err(e) = driver.disconnect() {
                    error!("Erreur deconnexion PLC : {}", e);
                }
            }
            info!("Worker d'upload arrêté proprement");
        });

        Self {
            config,
            compteur,
            etat,
            _surveillant: surveillant,
            _serveur_handle: serveur_handle,
            worker_handle: Some(worker_handle),
            arret,
            touche_quitter,
        }
    }

    pub fn lancer(self) {
        let plein_ecran = self.config.interface.plein_ecran;

        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Surveillance FTP + PIIA")
                .with_inner_size([1024.0, 768.0])
                .with_fullscreen(plein_ecran),
            ..Default::default()
        };

        if let Err(e) = eframe::run_native(
            "Surveillance FTP + PIIA",
            options,
            Box::new(|_cc| Ok(Box::new(self))),
        ) {
            error!("Erreur lancement interface : {}", e);
        }
    }
}

impl Drop for ApplicationGUI {
    fn drop(&mut self) {
        info!("Arrêt de l'application...");
        self.arret.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
        info!("Tous les threads arrêtés");
    }
}

impl eframe::App for ApplicationGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Une seule prise de verrou
        let (statut_texte, statut_couleur, texture, resultat_piia) = {
            let mut etat = self.etat.lock().unwrap();
            if etat.ctx.is_none() {
                etat.ctx = Some(ctx.clone());
            }

            // Charger la texture si le worker a préparé des données d'image
            if let Some((nom, image_data)) = etat.image_a_charger.take() {
                if let Ok(image) = image::load_from_memory(&image_data) {
                    let rgba = image.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let pixels = rgba.into_raw();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                    let texture =
                        ctx.load_texture(&nom, color_image, egui::TextureOptions::LINEAR);
                    etat.image_texture = Some(texture);
                }
            }

            (
                etat.statut.texte.clone(),
                etat.statut.couleur,
                etat.image_texture.clone(),
                etat.resultat_piia.clone(),
            )
        };

        // Touche configurable pour quitter
        if ctx.input(|i| i.key_pressed(self.touche_quitter)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Couleur de fond sombre
        let frame_bg = egui::Frame::new().fill(egui::Color32::from_rgb(26, 26, 26));

        let touche_nom = &self.config.interface.touche_quitter;

        // --- Barre du haut ---
        egui::TopBottomPanel::top("barre_haut")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 17, 17))
                    .inner_margin(egui::Margin::symmetric(15, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Indicateur OK/NOK bien visible
                    if let Some(ref res) = resultat_piia {
                        let (badge_text, badge_color) = if res.piece_ok {
                            (" OK ", egui::Color32::from_rgb(0, 180, 0))
                        } else {
                            (" NOK ", egui::Color32::from_rgb(220, 0, 0))
                        };

                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(60.0, 28.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(
                            rect,
                            6.0,
                            badge_color,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            badge_text.trim(),
                            egui::FontId::proportional(18.0),
                            egui::Color32::WHITE,
                        );
                        ui.add_space(10.0);
                    }

                    ui.label(
                        egui::RichText::new(&statut_texte)
                            .color(statut_couleur)
                            .size(14.0)
                            .strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(
                                egui::RichText::new("RAZ Compteur")
                                    .color(egui::Color32::WHITE)
                                    .size(11.0),
                            )
                            .clicked()
                        {
                            self.compteur.remettre_a_zero();
                        }

                        ui.add_space(15.0);

                        ui.label(
                            egui::RichText::new(format!(
                                "Envoyées : {}  |  Supprimées : {}",
                                self.compteur.total_envoye(),
                                self.compteur.total_supprime()
                            ))
                            .color(egui::Color32::from_rgb(0, 170, 255))
                            .size(13.0),
                        );
                    });
                });
            });

        // --- Barre du bas ---
        egui::TopBottomPanel::bottom("barre_bas")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 17, 17))
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let info_text = format!(
                        "FTP :{}  |  Envoi vers {}  |  Dossier : {}  |  {} = quitter",
                        self.config.serveur_ftp.port_ecoute,
                        self.config.ftp_client.host,
                        self.config.surveillance.repertoire_surveille.display(),
                        touche_nom,
                    );
                    ui.label(
                        egui::RichText::new(info_text)
                            .color(egui::Color32::from_rgb(119, 119, 119))
                            .size(10.0),
                    );

                    if self.config.piia.actif {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "PIIA: {} (timeout {}ms)",
                                self.config.piia.url, self.config.piia.timeout_ms
                            ))
                            .color(egui::Color32::from_rgb(0, 170, 255))
                            .size(10.0),
                        );
                    }

                    // Afficher le temps de traitement PIIA
                    if let Some(ref res) = resultat_piia {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("PIIA: {}ms", res.processing_time_ms))
                                    .color(egui::Color32::from_rgb(170, 170, 170))
                                    .size(10.0),
                            );
                        });
                    }
                });
            });

        // --- Zone image centrale ---
        egui::CentralPanel::default().frame(frame_bg).show(ctx, |ui| {
            if let Some(ref texture) = texture {
                let available = ui.available_size();
                let tex_size = texture.size_vec2();
                let ratio = (available.x / tex_size.x).min(available.y / tex_size.y);
                let display_size = egui::vec2(tex_size.x * ratio, tex_size.y * ratio);

                ui.centered_and_justified(|ui| {
                    ui.image(egui::load::SizedTexture::new(texture.id(), display_size));
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("Aucune image")
                            .color(egui::Color32::from_rgb(85, 85, 85))
                            .size(24.0),
                    );
                });
            }
        });

        // Refresh régulier
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

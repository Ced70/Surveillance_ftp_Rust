use crate::client_ftp::ClientFTP;
use crate::compteur::CompteurPersistant;
use crate::config::AppConfig;
use crate::serveur_ftp::demarrer_serveur_ftp;
use crate::surveillant::{attendre_fichier_complet, SurveillantImages};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Message de statut affiché dans l'interface
struct Statut {
    texte: String,
    couleur: egui::Color32,
}

/// État partagé entre le thread worker et l'interface
struct EtatPartage {
    statut: Statut,
    image_texture: Option<egui::TextureHandle>,
    /// Données brutes d'une image à charger comme texture (nom, bytes partagés)
    image_a_charger: Option<(String, Arc<[u8]>)>,
    /// Contexte egui pour demander un repaint depuis le worker
    ctx: Option<egui::Context>,
}

pub struct ApplicationGUI {
    config: AppConfig,
    compteur: CompteurPersistant,
    file_receiver: Receiver<PathBuf>,
    _file_sender: Sender<PathBuf>,
    etat: Arc<Mutex<EtatPartage>>,
    _surveillant: Option<SurveillantImages>,
    _serveur_handle: Option<std::thread::JoinHandle<()>>,
    upload_sender: Sender<PathBuf>,
    _worker_handle: Option<std::thread::JoinHandle<()>>,
}

impl ApplicationGUI {
    pub fn new(config: AppConfig) -> Self {
        let compteur_path = std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("compteur.json");
        let compteur = CompteurPersistant::new(&compteur_path);

        let (sender, receiver) = crossbeam_channel::unbounded();

        // Créer le répertoire surveillé
        let _ = std::fs::create_dir_all(&config.surveillance.repertoire_surveille);

        // Démarrer le serveur FTP intégré
        let serveur_handle = demarrer_serveur_ftp(
            &config.serveur_ftp,
            &config.surveillance.repertoire_surveille,
        );

        // Démarrer la surveillance watchdog
        let surveillant = match SurveillantImages::new(
            &config.surveillance.repertoire_surveille,
            config.surveillance.extensions.clone(),
            sender.clone(),
        ) {
            Ok(s) => Some(s),
            Err(e) => {
                error!("Impossible de démarrer le watcher : {}", e);
                None
            }
        };

        let etat = Arc::new(Mutex::new(EtatPartage {
            statut: Statut {
                texte: "En attente de nouvelles images...".to_string(),
                couleur: egui::Color32::from_rgb(0, 204, 0),
            },
            image_texture: None,
            image_a_charger: None,
            ctx: None,
        }));

        // Canal pour le worker d'upload sérialisé
        let (upload_sender, upload_receiver) = crossbeam_channel::unbounded::<PathBuf>();

        // Le worker possède directement le ClientFTP
        let mut client_ftp = ClientFTP::new(config.ftp_client.clone(), compteur.clone());
        let worker_etat = etat.clone();
        let worker_config = config.clone();

        // Créer le dossier d'envoi une seule fois si configuré
        if let Some(ref dossier) = config.surveillance.dossier_envoye {
            let _ = std::fs::create_dir_all(dossier);
        }

        let worker_handle = std::thread::spawn(move || {
            for chemin in upload_receiver {
                let nom = chemin
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Attendre que le fichier soit complètement écrit
                if !attendre_fichier_complet(&chemin, Duration::from_secs(15)) {
                    warn!("Fichier incomplet après timeout : {}", chemin.display());
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

                // Lire le fichier une seule fois, partager via Arc
                let image_data: Arc<[u8]> = match std::fs::read(&chemin) {
                    Ok(data) => data.into(),
                    Err(e) => {
                        error!("Impossible de lire {} : {}", chemin.display(), e);
                        continue;
                    }
                };

                // Préparer l'affichage (Arc::clone = copie du pointeur, pas des données)
                {
                    let mut etat = worker_etat.lock().unwrap();
                    etat.image_a_charger = Some((nom.clone(), Arc::clone(&image_data)));
                    etat.statut = Statut {
                        texte: format!("Envoi en cours : {}", nom),
                        couleur: egui::Color32::from_rgb(255, 170, 0),
                    };
                    if let Some(ctx) = &etat.ctx {
                        ctx.request_repaint();
                    }
                }

                // Envoyer via FTP (réutilise la connexion existante)
                let succes = client_ftp.envoyer_donnees(&nom, &image_data);

                {
                    let mut etat = worker_etat.lock().unwrap();
                    if succes {
                        etat.statut = Statut {
                            texte: format!("Envoyé : {}", nom),
                            couleur: egui::Color32::from_rgb(0, 204, 0),
                        };
                        if let Some(ref dossier) = worker_config.surveillance.dossier_envoye {
                            if let Some(nom_fichier) = chemin.file_name() {
                                let destination = dossier.join(nom_fichier);
                                if let Err(e) = std::fs::rename(&chemin, &destination) {
                                    error!(
                                        "Impossible de déplacer {} : {}",
                                        chemin.display(),
                                        e
                                    );
                                } else {
                                    info!("Image déplacée vers : {}", destination.display());
                                }
                            }
                        }
                    } else {
                        etat.statut = Statut {
                            texte: format!("Échec envoi : {}", nom),
                            couleur: egui::Color32::from_rgb(255, 68, 68),
                        };
                    }
                    if let Some(ctx) = &etat.ctx {
                        ctx.request_repaint();
                    }
                }
            }
        });

        Self {
            config,
            compteur,
            file_receiver: receiver,
            _file_sender: sender,
            etat,
            _surveillant: surveillant,
            _serveur_handle: serveur_handle,
            upload_sender,
            _worker_handle: Some(worker_handle),
        }
    }

    pub fn lancer(self) {
        let plein_ecran = self.config.interface.plein_ecran;

        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Surveillance FTP")
                .with_inner_size([1024.0, 768.0])
                .with_fullscreen(plein_ecran),
            ..Default::default()
        };

        if let Err(e) = eframe::run_native(
            "Surveillance FTP",
            options,
            Box::new(|_cc| Ok(Box::new(self))),
        ) {
            error!("Erreur lancement interface : {}", e);
        }
    }
}

impl eframe::App for ApplicationGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Une seule prise de verrou pour : ctx, texture, statut
        let (statut_texte, statut_couleur, texture) = {
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

            // Copier les données nécessaires au rendu, puis libérer le verrou
            (
                etat.statut.texte.clone(),
                etat.statut.couleur,
                etat.image_texture.clone(),
            )
        };
        // Verrou libéré ici — le worker n'est plus bloqué pendant le rendu

        // Touche Escape pour quitter
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Transférer les nouvelles images vers le worker d'upload
        while let Ok(chemin) = self.file_receiver.try_recv() {
            if let Err(e) = self.upload_sender.send(chemin) {
                error!("Erreur envoi vers worker : {}", e);
            }
        }

        // Couleur de fond sombre
        let frame_bg = egui::Frame::new().fill(egui::Color32::from_rgb(26, 26, 26));

        // --- Barre du haut ---
        egui::TopBottomPanel::top("barre_haut")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 17, 17))
                    .inner_margin(egui::Margin::symmetric(15, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
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
                let info_text = format!(
                    "Écoute FTP :{}  |  Envoi vers {}  |  Dossier : {}  |  Échap = quitter",
                    self.config.serveur_ftp.port_ecoute,
                    self.config.ftp_client.host,
                    self.config.surveillance.repertoire_surveille.display()
                );
                ui.label(
                    egui::RichText::new(info_text)
                        .color(egui::Color32::from_rgb(119, 119, 119))
                        .size(10.0),
                );
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

        // Refresh régulier pour vérifier les nouvelles images
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

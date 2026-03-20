use crossbeam_channel::Sender;
use log::{error, info};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Surveille un répertoire et envoie les chemins des nouvelles images dans le channel.
pub struct SurveillantImages {
    _watcher: RecommendedWatcher,
}

/// Délai minimum entre deux envois du même fichier dans le channel (anti-rebond).
const DELAI_ANTI_REBOND: Duration = Duration::from_secs(2);

impl SurveillantImages {
    pub fn new(
        repertoire: &Path,
        extensions: Vec<String>,
        sender: Sender<PathBuf>,
    ) -> Result<Self, notify::Error> {
        let derniers_envois: std::sync::Arc<Mutex<HashMap<PathBuf, Instant>>> =
            std::sync::Arc::new(Mutex::new(HashMap::new()));
        let envois_clone = derniers_envois.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_)
                    ) {
                        for chemin in event.paths {
                            let ext = chemin
                                .extension()
                                .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                                .unwrap_or_default();

                            if !extensions.contains(&ext) {
                                continue;
                            }

                            // Anti-rebond : ignorer si envoyé récemment
                            {
                                let mut envois = envois_clone.lock().unwrap();
                                let now = Instant::now();
                                if let Some(dernier) = envois.get(&chemin) {
                                    if now.duration_since(*dernier) < DELAI_ANTI_REBOND {
                                        continue;
                                    }
                                }
                                envois.insert(chemin.clone(), now);

                                // Purger les entrées obsolètes pour éviter la fuite mémoire
                                if envois.len() > 500 {
                                    envois.retain(|_, instant| {
                                        now.duration_since(*instant) < Duration::from_secs(60)
                                    });
                                }
                            }

                            info!("Nouvelle image détectée (watcher) : {}", chemin.display());
                            if let Err(e) = sender.send(chemin) {
                                error!("Erreur envoi dans le channel : {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Erreur watcher : {}", e);
                }
            })?;

        watcher.watch(repertoire, RecursiveMode::NonRecursive)?;
        info!(
            "Surveillance watcher démarrée sur : {}",
            repertoire.display()
        );

        Ok(Self {
            _watcher: watcher,
        })
    }
}

/// Attend que le fichier soit complètement écrit sur disque.
/// Vérifie que la taille est stable pendant 2 lectures consécutives espacées de 150ms.
pub fn attendre_fichier_complet(chemin: &Path, timeout: Duration) -> bool {
    let debut = Instant::now();
    let mut taille_precedente: i64 = -1;
    let mut lectures_stables: u32 = 0;
    const LECTURES_REQUISES: u32 = 2;

    while debut.elapsed() < timeout {
        match std::fs::metadata(chemin) {
            Ok(meta) => {
                let taille = meta.len() as i64;
                if taille == taille_precedente && taille > 0 {
                    lectures_stables += 1;
                    if lectures_stables >= LECTURES_REQUISES {
                        return true;
                    }
                } else {
                    lectures_stables = 0;
                }
                taille_precedente = taille;
            }
            Err(_) => {
                lectures_stables = 0;
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    // Retourner true si le fichier existe et a une taille > 0
    std::fs::metadata(chemin)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

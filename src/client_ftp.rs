use crate::compteur::CompteurPersistant;
use crate::config::FtpClientConfig;
use log::{error, info, warn};
use std::io::Cursor;
use suppaftp::FtpStream;

/// Client FTP pour l'envoi d'images vers le serveur distant.
/// Maintient une connexion persistante et un cache du nombre de fichiers distants.
pub struct ClientFTP {
    config: FtpClientConfig,
    compteur: CompteurPersistant,
    connexion: Option<FtpStream>,
    /// Cache du nombre de fichiers distants. None = inconnu, forcer un listing.
    nb_fichiers_distants: Option<usize>,
}

impl ClientFTP {
    pub fn new(config: FtpClientConfig, compteur: CompteurPersistant) -> Self {
        Self {
            config,
            compteur,
            connexion: None,
            nb_fichiers_distants: None,
        }
    }

    /// Vérifie que la connexion est active, sinon en crée une nouvelle.
    fn assurer_connexion(&mut self) -> Result<(), suppaftp::FtpError> {
        let est_connecte = match &mut self.connexion {
            Some(ftp) => ftp.pwd().is_ok(),
            None => false,
        };

        if !est_connecte {
            if let Some(mut ancien) = self.connexion.take() {
                let _ = ancien.quit();
            }
            // Compteur inconnu après reconnexion
            self.nb_fichiers_distants = None;

            let addr = format!("{}:{}", self.config.host, self.config.port);
            let mut ftp = FtpStream::connect(&addr)?;
            ftp.login(&self.config.utilisateur, &self.config.mot_de_passe)?;
            // Toujours en mode binaire pour les images
            ftp.transfer_type(suppaftp::types::FileType::Binary)?;

            if ftp.cwd(&self.config.dossier_distant).is_err() {
                self.creer_dossier_recursif(&mut ftp, &self.config.dossier_distant)?;
                ftp.cwd(&self.config.dossier_distant)?;
            }

            self.connexion = Some(ftp);
        }

        Ok(())
    }

    fn creer_dossier_recursif(
        &self,
        ftp: &mut FtpStream,
        chemin: &str,
    ) -> Result<(), suppaftp::FtpError> {
        let mut chemin_courant = String::new();
        for partie in chemin.trim_matches('/').split('/') {
            chemin_courant.push('/');
            chemin_courant.push_str(partie);
            if ftp.cwd(&chemin_courant).is_err() {
                let _ = ftp.mkdir(&chemin_courant);
            }
        }
        Ok(())
    }

    /// Envoie des données déjà lues en mémoire vers le serveur FTP.
    /// Réutilise la connexion existante et cache le nombre de fichiers distants.
    pub fn envoyer_donnees(&mut self, nom_fichier: &str, donnees: &[u8]) -> bool {
        if let Err(e) = self.assurer_connexion() {
            error!("Erreur connexion FTP pour {} : {}", nom_fichier, e);
            return false;
        }

        // Destructuration pour emprunts disjoints
        let Self {
            config,
            compteur,
            connexion,
            nb_fichiers_distants,
        } = self;
        let ftp = connexion.as_mut().unwrap();

        // Gestion FIFO : ne lister que si on approche de la limite ou si le cache est inconnu
        let besoin_fifo = match *nb_fichiers_distants {
            None => true,
            Some(n) => n + 1 >= config.max_images,
        };

        if besoin_fifo {
            let mut fichiers: Vec<String> = match ftp.nlst(Some(&config.dossier_distant)) {
                Ok(liste) => liste
                    .into_iter()
                    .filter(|n| n != "." && n != "..")
                    .collect(),
                Err(e) => {
                    warn!("Impossible de lister les fichiers distants : {}", e);
                    Vec::new()
                }
            };

            let mut nb_supprimes: u64 = 0;
            while fichiers.len() >= config.max_images {
                if let Some(ancien) = fichiers.first().cloned() {
                    let chemin_distant = format!("{}/{}", config.dossier_distant, ancien);
                    match ftp.rm(&chemin_distant) {
                        Ok(_) => {
                            info!("Image ancienne supprimée : {}", ancien);
                            nb_supprimes += 1;
                            fichiers.remove(0);
                        }
                        Err(e) => {
                            error!("Impossible de supprimer {} : {}", ancien, e);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }

            // Incrémenter le compteur de suppression en une seule écriture disque
            if nb_supprimes > 0 {
                compteur.incrementer_supprime_par(nb_supprimes);
            }

            *nb_fichiers_distants = Some(fichiers.len());
        }

        // Envoi depuis les données en mémoire
        let mut cursor = Cursor::new(donnees);
        match ftp.put_file(nom_fichier, &mut cursor) {
            Ok(_) => {
                compteur.incrementer_envoye();
                *nb_fichiers_distants = nb_fichiers_distants.map(|n| n + 1);
                info!("Image envoyée vers distant : {}", nom_fichier);
                true
            }
            Err(e) => {
                error!("Erreur envoi FTP pour {} : {}", nom_fichier, e);
                // Connexion probablement cassée
                if let Some(mut ftp) = connexion.take() {
                    let _ = ftp.quit();
                }
                *nb_fichiers_distants = None;
                false
            }
        }
    }
}

impl Drop for ClientFTP {
    fn drop(&mut self) {
        if let Some(mut ftp) = self.connexion.take() {
            let _ = ftp.quit();
        }
    }
}

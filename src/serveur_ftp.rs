use crate::config::ServeurFtpConfig;
use log::info;
use std::path::PathBuf;
use std::sync::Arc;

/// Authentificateur par identifiants pour le serveur FTP intégré.
#[derive(Debug)]
struct IdentifiantsAuthenticator {
    utilisateur: String,
    mot_de_passe: String,
}

#[async_trait::async_trait]
impl libunftp::auth::Authenticator<libunftp::auth::DefaultUser> for IdentifiantsAuthenticator {
    async fn authenticate(
        &self,
        username: &str,
        creds: &libunftp::auth::Credentials,
    ) -> Result<libunftp::auth::DefaultUser, libunftp::auth::AuthenticationError> {
        if username != self.utilisateur {
            return Err(libunftp::auth::AuthenticationError::BadUser);
        }
        match &creds.password {
            Some(pass) if pass == &self.mot_de_passe => Ok(libunftp::auth::DefaultUser {}),
            _ => Err(libunftp::auth::AuthenticationError::BadPassword),
        }
    }
}

/// Démarre le serveur FTP intégré dans un runtime tokio dédié.
/// Les nouveaux fichiers sont détectés via le watcher (notify) sur le répertoire.
pub fn demarrer_serveur_ftp(
    config: &ServeurFtpConfig,
    repertoire: &PathBuf,
) -> Option<std::thread::JoinHandle<()>> {
    if !config.actif {
        info!("Serveur FTP intégré désactivé dans la configuration");
        return None;
    }

    let addr = format!("{}:{}", config.adresse_ecoute, config.port_ecoute);
    let repertoire = repertoire.clone();
    let passif_min = config.port_passif_min;
    let passif_max = config.port_passif_max;
    let masquerade = config.masquerade_address.clone();
    let anonyme = config.anonyme;
    let utilisateur = config.utilisateur.clone();
    let mot_de_passe = config.mot_de_passe.clone();

    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Impossible de créer le runtime tokio pour le serveur FTP");

        rt.block_on(async move {
            let rep_str = repertoire
                .to_str()
                .expect("Chemin du répertoire invalide (non-UTF8)")
                .to_string();

            use unftp_sbe_fs::ServerExt;
            let mut server = libunftp::Server::with_fs(rep_str.clone())
                .passive_ports(passif_min..passif_max);

            // Authentification : anonyme ou par identifiants
            if !anonyme {
                if let (Some(user), Some(pass)) = (utilisateur, mot_de_passe) {
                    info!("Serveur FTP : authentification par identifiants ({})", user);
                    server = server.authenticator(Arc::new(IdentifiantsAuthenticator {
                        utilisateur: user,
                        mot_de_passe: pass,
                    }));
                } else {
                    info!("Serveur FTP : accès anonyme (pas d'identifiants configurés)");
                    server = server
                        .authenticator(Arc::new(libunftp::auth::AnonymousAuthenticator));
                }
            } else {
                info!("Serveur FTP : accès anonyme activé");
                server =
                    server.authenticator(Arc::new(libunftp::auth::AnonymousAuthenticator));
            }

            // Masquerade address pour le mode passif derrière NAT
            if let Some(ref masq) = masquerade {
                if let Ok(ip) = masq.parse::<std::net::Ipv4Addr>() {
                    server = server.passive_host(ip);
                    info!("Serveur FTP : masquerade address = {}", masq);
                }
            }

            let server = server.build().unwrap();

            info!(
                "Serveur FTP en écoute sur {} (répertoire : {})",
                addr, &rep_str
            );

            if let Err(e) = server.listen(addr).await {
                log::error!("Erreur serveur FTP : {}", e);
            }
        });
    });

    Some(handle)
}

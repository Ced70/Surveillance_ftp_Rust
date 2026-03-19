use crate::config::ServeurFtpConfig;
use log::info;
use std::path::PathBuf;
use std::sync::Arc;

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
            let server = libunftp::Server::with_fs(rep_str.clone())
            .passive_ports(passif_min..passif_max)
            .authenticator(Arc::new(libunftp::auth::AnonymousAuthenticator))
            .build()
            .unwrap();

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

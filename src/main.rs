mod client_ftp;
mod compteur;
mod config;
mod gui;
mod serveur_ftp;
mod surveillant;

use config::AppConfig;
use gui::ApplicationGUI;
use log::{error, info};
use std::path::Path;

fn main() {
    // Initialiser le logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            use std::io::Write;
            let now = chrono::Local::now();
            writeln!(
                buf,
                "{} [{}] {}",
                now.format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .init();

    // Trouver le fichier de configuration
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let config_path = exe_dir.join("config.ini");
    let config_path = if config_path.exists() {
        config_path
    } else {
        // Chercher aussi dans le répertoire courant
        let cwd_config = Path::new("config.ini");
        if cwd_config.exists() {
            cwd_config.to_path_buf()
        } else {
            error!("Fichier de configuration introuvable : config.ini");
            eprintln!("Erreur : fichier config.ini introuvable.");
            eprintln!("Placez config.ini à côté de l'exécutable ou dans le répertoire courant.");
            std::process::exit(1);
        }
    };

    info!("Chargement de la configuration : {}", config_path.display());

    let config = match AppConfig::charger(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("{}", e);
            eprintln!("Erreur de configuration : {}", e);
            std::process::exit(1);
        }
    };

    info!("Démarrage de Surveillance FTP");
    let app = ApplicationGUI::new(config);
    app.lancer();
    info!("Application arrêtée");
}

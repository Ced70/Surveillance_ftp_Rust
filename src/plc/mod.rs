pub mod pccc;
pub mod allen_bradley;

use log::warn;

use crate::config::AutomateConfig;
use self::allen_bradley::AllenBradleyDriver;

/// Resultat simplifie a ecrire vers l'automate
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResultatPiece {
    Ok,     // piece bonne
    Nok,    // piece defectueuse ou absente
    Timeout, // pas de reponse PIIA dans le delai
}

/// Trait pour les drivers PLC (version synchrone)
pub trait PlcDriver: Send {
    /// Connexion a l'automate
    fn connect(&mut self) -> anyhow::Result<()>;

    /// Deconnexion
    fn disconnect(&mut self) -> anyhow::Result<()>;

    /// Ecriture du resultat vers l'automate
    fn write_result(&mut self, resultat: ResultatPiece) -> anyhow::Result<()>;

    /// Verifie si la connexion est active
    #[allow(dead_code)]
    fn is_connected(&self) -> bool;

    /// Nom du driver (pour logs)
    fn name(&self) -> &str;
}

/// Cree le driver PLC adapte a la configuration
pub fn creer_driver(config: &AutomateConfig) -> Option<Box<dyn PlcDriver>> {
    if !config.actif {
        return None;
    }

    match config.driver_type.to_lowercase().as_str() {
        "allen_bradley" | "allenbradley" | "ab" => {
            let address = config.address.as_deref().unwrap_or("192.168.1.10");
            let result_tag = config.result_tag.as_deref().unwrap_or("N7:0");
            let heartbeat_tag = config.heartbeat_tag.as_deref().unwrap_or("N7:1");

            match AllenBradleyDriver::new(address, result_tag, heartbeat_tag) {
                Ok(driver) => Some(Box::new(driver)),
                Err(e) => {
                    log::error!("Erreur creation driver Allen-Bradley : {}", e);
                    None
                }
            }
        }
        "fichier" | "file" => {
            let path = config.fichier_resultat.as_deref().unwrap_or("/tmp/surveillance_resultat.txt");
            Some(Box::new(FichierDriver::new(path)))
        }
        "tcp" => {
            let addr = config.tcp_adresse.as_deref().unwrap_or("192.168.1.200:5000");
            Some(Box::new(TcpDriver::new(addr)))
        }
        "none" | "" => None,
        autre => {
            warn!("Type d'automate inconnu : '{}' (disponibles: allen_bradley, fichier, tcp, none)", autre);
            None
        }
    }
}

// === Driver fichier (simple, pour debug/test) ===

struct FichierDriver {
    chemin: String,
}

impl FichierDriver {
    fn new(chemin: &str) -> Self {
        log::info!("Automate fichier : {}", chemin);
        Self { chemin: chemin.to_string() }
    }
}

impl PlcDriver for FichierDriver {
    fn connect(&mut self) -> anyhow::Result<()> { Ok(()) }
    fn disconnect(&mut self) -> anyhow::Result<()> { Ok(()) }

    fn write_result(&mut self, resultat: ResultatPiece) -> anyhow::Result<()> {
        let texte = match resultat {
            ResultatPiece::Ok => "OK",
            ResultatPiece::Nok => "NOK",
            ResultatPiece::Timeout => "TIMEOUT",
        };
        std::fs::write(&self.chemin, texte)?;
        log::info!("Automate fichier: {} -> {}", texte, self.chemin);
        Ok(())
    }

    fn is_connected(&self) -> bool { true }
    fn name(&self) -> &str { "fichier" }
}

// === Driver TCP simple ===

struct TcpDriver {
    adresse: String,
}

impl TcpDriver {
    fn new(adresse: &str) -> Self {
        log::info!("Automate TCP : {}", adresse);
        Self { adresse: adresse.to_string() }
    }
}

impl PlcDriver for TcpDriver {
    fn connect(&mut self) -> anyhow::Result<()> { Ok(()) }
    fn disconnect(&mut self) -> anyhow::Result<()> { Ok(()) }

    fn write_result(&mut self, resultat: ResultatPiece) -> anyhow::Result<()> {
        let texte = match resultat {
            ResultatPiece::Ok => "OK\n",
            ResultatPiece::Nok => "NOK\n",
            ResultatPiece::Timeout => "TIMEOUT\n",
        };
        let addr: std::net::SocketAddr = self.adresse.parse()
            .unwrap_or_else(|_| "127.0.0.1:5000".parse().unwrap());
        match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)) {
            Ok(mut stream) => {
                use std::io::Write;
                stream.write_all(texte.as_bytes())?;
                log::info!("Automate TCP: {} -> {}", texte.trim(), self.adresse);
            }
            Err(e) => {
                log::warn!("Automate TCP non joignable {} : {}", self.adresse, e);
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool { true }
    fn name(&self) -> &str { "tcp" }
}

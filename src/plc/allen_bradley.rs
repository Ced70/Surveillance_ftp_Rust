use std::sync::atomic::{AtomicBool, AtomicI16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, info, warn};

use super::pccc::{PcccAddress, PcccClient};
use super::{PlcDriver, ResultatPiece};

/// Driver Allen-Bradley via PCCC over EtherNet/IP (version synchrone).
///
/// Compatible avec les automates MicroLogix / SLC-500.
/// Meme protocole et memes valeurs que PIIA :
///
/// | Tag              | Valeur                                         |
/// |------------------|------------------------------------------------|
/// | `result_tag`     | 1 = OK, 2 = NOK, 0 = arret communication      |
/// | `heartbeat_tag`  | Compteur 1..32767, incremente chaque seconde   |
pub struct AllenBradleyDriver {
    address: String,
    result_tag_str: String,
    heartbeat_tag_str: String,
    result_address: PcccAddress,
    heartbeat_address: PcccAddress,
    client: Arc<Mutex<Option<PcccClient>>>,
    connected: Arc<AtomicBool>,
    heartbeat: Arc<AtomicI16>,
    heartbeat_thread: Option<std::thread::JoinHandle<()>>,
}

impl AllenBradleyDriver {
    pub fn new(address: &str, result_tag: &str, heartbeat_tag: &str) -> anyhow::Result<Self> {
        let result_address = PcccAddress::parse(result_tag)?;
        let heartbeat_address = PcccAddress::parse(heartbeat_tag)?;

        info!(
            "[AB-PCCC] Driver cree : address={}, result='{}', heartbeat='{}'",
            address, result_tag, heartbeat_tag
        );

        Ok(Self {
            address: address.to_string(),
            result_tag_str: result_tag.to_string(),
            heartbeat_tag_str: heartbeat_tag.to_string(),
            result_address,
            heartbeat_address,
            client: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            heartbeat: Arc::new(AtomicI16::new(0)),
            heartbeat_thread: None,
        })
    }

    /// Demarre le thread de heartbeat (incremente toutes les secondes)
    fn start_heartbeat(&mut self) {
        let client = Arc::clone(&self.client);
        let connected = Arc::clone(&self.connected);
        let heartbeat = Arc::clone(&self.heartbeat);
        let address = self.heartbeat_address.clone();
        let tag_str = self.heartbeat_tag_str.clone();

        self.heartbeat_thread = Some(std::thread::spawn(move || {
            debug!("[AB-PCCC] Thread heartbeat demarre (tag={})", tag_str);

            while connected.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));

                if !connected.load(Ordering::SeqCst) {
                    break;
                }

                // Incrementer le heartbeat (1..32767 puis reboucle a 1)
                let current = heartbeat.fetch_add(1, Ordering::SeqCst);
                let value = if current >= i16::MAX {
                    heartbeat.store(1, Ordering::SeqCst);
                    1i16
                } else {
                    current.wrapping_add(1)
                };

                let mut guard = client.lock().unwrap();
                if let Some(ref mut c) = *guard {
                    if let Err(e) = c.write_int(&address, value) {
                        warn!("[AB-PCCC] Erreur heartbeat: {}", e);
                        // Connexion probablement perdue
                        connected.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
            debug!("[AB-PCCC] Thread heartbeat arrete");
        }));
    }
}

impl PlcDriver for AllenBradleyDriver {
    fn connect(&mut self) -> anyhow::Result<()> {
        info!("[AB-PCCC] Connexion a {}...", self.address);

        let client = PcccClient::connect(&self.address, Duration::from_secs(5))?;

        *self.client.lock().unwrap() = Some(client);
        self.connected.store(true, Ordering::SeqCst);
        self.heartbeat.store(0, Ordering::SeqCst);

        // Demarrer le heartbeat
        self.start_heartbeat();

        info!(
            "[AB-PCCC] Connecte a {} — result='{}', heartbeat='{}'",
            self.address, self.result_tag_str, self.heartbeat_tag_str
        );
        Ok(())
    }

    fn disconnect(&mut self) -> anyhow::Result<()> {
        info!("[AB-PCCC] Deconnexion de {}...", self.address);
        self.connected.store(false, Ordering::SeqCst);

        // Attendre la fin du thread heartbeat
        if let Some(handle) = self.heartbeat_thread.take() {
            let _ = handle.join();
        }

        // Remettre heartbeat et resultat a 0
        let mut guard = self.client.lock().unwrap();
        if let Some(ref mut client) = *guard {
            if let Err(e) = client.write_int(&self.heartbeat_address, 0) {
                warn!("[AB-PCCC] Erreur RAZ heartbeat: {}", e);
            }
            if let Err(e) = client.write_int(&self.result_address, 0) {
                warn!("[AB-PCCC] Erreur RAZ resultat: {}", e);
            }
        }
        // Drop du client ferme la connexion
        guard.take();

        info!("[AB-PCCC] Deconnecte de {}", self.address);
        Ok(())
    }

    fn write_result(&mut self, resultat: ResultatPiece) -> anyhow::Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            // Tenter une reconnexion
            warn!("[AB-PCCC] Non connecte, tentative de reconnexion...");
            self.connect()?;
        }

        // Memes valeurs que PIIA : 1 = OK, 2 = NOK
        let status: i16 = match resultat {
            ResultatPiece::Ok => 1,
            ResultatPiece::Nok => 2,
            ResultatPiece::Timeout => 2, // Timeout = NOK pour securite
        };

        let mut guard = self.client.lock().unwrap();
        let client = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Client PCCC non initialise"))?;

        client.write_int(&self.result_address, status)?;

        debug!(
            "[AB-PCCC] Resultat ecrit: {} = {} ({})",
            self.result_tag_str,
            status,
            match resultat {
                ResultatPiece::Ok => "OK",
                ResultatPiece::Nok => "NOK",
                ResultatPiece::Timeout => "TIMEOUT->NOK",
            }
        );

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn name(&self) -> &str {
        "allen-bradley"
    }
}

impl Drop for AllenBradleyDriver {
    fn drop(&mut self) {
        if self.connected.load(Ordering::SeqCst) {
            let _ = self.disconnect();
        }
    }
}

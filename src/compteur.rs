use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CompteurData {
    total_envoye: u64,
    total_supprime: u64,
}

/// Compteur persistant sauvegardé sur disque (écriture atomique).
#[derive(Clone)]
pub struct CompteurPersistant {
    inner: Arc<Mutex<CompteurInner>>,
}

struct CompteurInner {
    chemin: PathBuf,
    total_envoye: u64,
    total_supprime: u64,
}

impl CompteurPersistant {
    pub fn new(chemin: &Path) -> Self {
        let data = Self::charger(chemin);
        Self {
            inner: Arc::new(Mutex::new(CompteurInner {
                chemin: chemin.to_path_buf(),
                total_envoye: data.total_envoye,
                total_supprime: data.total_supprime,
            })),
        }
    }

    fn charger(chemin: &Path) -> CompteurData {
        match fs::read_to_string(chemin) {
            Ok(contenu) => serde_json::from_str(&contenu).unwrap_or(CompteurData {
                total_envoye: 0,
                total_supprime: 0,
            }),
            Err(_) => {
                let data = CompteurData {
                    total_envoye: 0,
                    total_supprime: 0,
                };
                let _ = Self::sauvegarder_data(chemin, &data);
                data
            }
        }
    }

    fn sauvegarder_data(chemin: &Path, data: &CompteurData) -> Result<(), std::io::Error> {
        let tmp = chemin.with_extension("json.tmp");
        let contenu = serde_json::to_string(data)?;
        fs::write(&tmp, &contenu)?;
        fs::rename(&tmp, chemin)?;
        Ok(())
    }

    fn sauvegarder(inner: &CompteurInner) {
        let data = CompteurData {
            total_envoye: inner.total_envoye,
            total_supprime: inner.total_supprime,
        };
        if let Err(e) = Self::sauvegarder_data(&inner.chemin, &data) {
            log::error!("Erreur sauvegarde compteur : {}", e);
        }
    }

    pub fn incrementer_envoye(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.total_envoye += 1;
        Self::sauvegarder(&inner);
    }

    pub fn incrementer_supprime_par(&self, n: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.total_supprime += n;
        Self::sauvegarder(&inner);
    }

    pub fn remettre_a_zero(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.total_envoye = 0;
        inner.total_supprime = 0;
        Self::sauvegarder(&inner);
    }

    pub fn total_envoye(&self) -> u64 {
        self.inner.lock().unwrap().total_envoye
    }

    pub fn total_supprime(&self) -> u64 {
        self.inner.lock().unwrap().total_supprime
    }
}

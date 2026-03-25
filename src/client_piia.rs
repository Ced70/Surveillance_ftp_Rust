use crate::config::PiiaConfig;
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::time::Duration;

/// Resultat d'analyse retourne par PIIA
#[derive(Debug, Clone)]
pub struct ResultatAnalyse {
    /// "ok", "defect", "absent"
    pub global_status: String,
    /// true si la piece est OK
    pub piece_ok: bool,
    /// Image annotee (JPEG bytes) avec heatmap et bboxes
    pub image_annotee: Vec<u8>,
    /// Temps de traitement en ms
    pub processing_time_ms: u64,
    /// Resultats par module (serialise en JSON pour affichage)
    pub details: String,
}

/// Reponse JSON du serveur PIIA
#[derive(Deserialize)]
struct PiiaResponse {
    global_status: String,
    piece_ok: bool,
    annotated_image_base64: String,
    processing_time_ms: u64,
    #[serde(default)]
    module_results: serde_json::Value,
}

/// Client HTTP pour communiquer avec le serveur PIIA
pub struct ClientPIIA {
    config: PiiaConfig,
    client: reqwest::blocking::Client,
    url_analyse: String,
}

impl ClientPIIA {
    pub fn new(config: PiiaConfig) -> Self {
        let timeout = Duration::from_millis(config.timeout_ms);
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_millis(config.timeout_ms.min(2000)))
            .build()
            .expect("Impossible de creer le client HTTP");

        let url_analyse = format!("{}/api/external/analyse", config.url.trim_end_matches('/'));

        info!(
            "Client PIIA initialise : {} (timeout={}ms)",
            url_analyse, config.timeout_ms
        );

        Self {
            config,
            client,
            url_analyse,
        }
    }

    /// Envoie une image a PIIA pour analyse.
    /// Retourne None si timeout ou erreur (l'image est ignoree).
    pub fn analyser(&self, nom_fichier: &str, image_data: &[u8]) -> Option<ResultatAnalyse> {
        if !self.config.actif {
            return None;
        }

        debug!(
            "PIIA: envoi {} ({} octets) vers {}",
            nom_fichier,
            image_data.len(),
            self.url_analyse
        );

        let form = match reqwest::blocking::multipart::Form::new()
            .part(
                "image",
                reqwest::blocking::multipart::Part::bytes(image_data.to_vec())
                    .file_name(nom_fichier.to_string())
                    .mime_str("image/jpeg")
                    .ok()?,
            )
        {
            f => f,
        };

        let response = match self.client.post(&self.url_analyse).multipart(form).send() {
            Ok(resp) => resp,
            Err(e) => {
                if e.is_timeout() {
                    warn!(
                        "PIIA timeout ({}ms) pour {} - image ignoree",
                        self.config.timeout_ms, nom_fichier
                    );
                } else if e.is_connect() {
                    warn!("PIIA non joignable ({}) pour {}", self.config.url, nom_fichier);
                } else {
                    error!("Erreur HTTP PIIA pour {} : {}", nom_fichier, e);
                }
                return None;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            error!(
                "PIIA erreur {} pour {} : {}",
                status, nom_fichier, body
            );
            return None;
        }

        let piia_resp: PiiaResponse = match response.json() {
            Ok(r) => r,
            Err(e) => {
                error!("Erreur decodage reponse PIIA pour {} : {}", nom_fichier, e);
                return None;
            }
        };

        // Decoder l'image base64
        use base64::Engine;
        let image_annotee = match base64::engine::general_purpose::STANDARD
            .decode(&piia_resp.annotated_image_base64)
        {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("Erreur decodage base64 image PIIA pour {} : {}", nom_fichier, e);
                return None;
            }
        };

        let details = serde_json::to_string_pretty(&piia_resp.module_results)
            .unwrap_or_default();

        info!(
            "PIIA analyse {} : {} ({}ms, piece_ok={})",
            nom_fichier, piia_resp.global_status, piia_resp.processing_time_ms, piia_resp.piece_ok
        );

        Some(ResultatAnalyse {
            global_status: piia_resp.global_status,
            piece_ok: piia_resp.piece_ok,
            image_annotee,
            processing_time_ms: piia_resp.processing_time_ms,
            details,
        })
    }
}

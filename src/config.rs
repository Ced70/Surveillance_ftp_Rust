use ini::Ini;
use std::path::{Path, PathBuf};

/// Configuration de la section [surveillance]
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct SurveillanceConfig {
    pub repertoire_surveille: PathBuf,
    pub extensions: Vec<String>,
    /// Obsolete : les images sont maintenant supprimees apres traitement
    pub dossier_envoye: Option<PathBuf>,
}

/// Configuration de la section [serveur_ftp]
#[derive(Clone, Debug)]
pub struct ServeurFtpConfig {
    pub actif: bool,
    pub port_ecoute: u16,
    pub adresse_ecoute: String,
    pub utilisateur: Option<String>,
    pub mot_de_passe: Option<String>,
    pub anonyme: bool,
    pub masquerade_address: Option<String>,
    pub port_passif_min: u16,
    pub port_passif_max: u16,
}

/// Configuration de la section [ftp] (client distant)
#[derive(Clone, Debug)]
pub struct FtpClientConfig {
    pub host: String,
    pub port: u16,
    pub utilisateur: String,
    pub mot_de_passe: String,
    pub dossier_distant: String,
    pub max_images: usize,
    pub mode_passif: bool,
}

/// Configuration de la section [piia]
#[derive(Clone, Debug)]
pub struct PiiaConfig {
    pub actif: bool,
    pub url: String,
    pub timeout_ms: u64,
}

/// Configuration de la section [automate]
/// Compatible avec la configuration PLC de PIIA
#[derive(Clone, Debug)]
pub struct AutomateConfig {
    pub actif: bool,
    /// Type de driver : "allen_bradley", "fichier", "tcp", "none"
    pub driver_type: String,
    /// Adresse IP de l'automate (Allen-Bradley)
    pub address: Option<String>,
    /// Adresse PCCC du tag resultat (ex: "N7:0") — INT : 1=OK, 2=NOK
    pub result_tag: Option<String>,
    /// Adresse PCCC du tag heartbeat (ex: "N7:1") — INT : compteur 1..32767
    pub heartbeat_tag: Option<String>,
    /// Chemin du fichier resultat (mode fichier)
    pub fichier_resultat: Option<String>,
    /// Adresse:port du serveur TCP (mode tcp)
    pub tcp_adresse: Option<String>,
}

/// Configuration de la section [interface]
#[derive(Clone, Debug)]
pub struct InterfaceConfig {
    pub plein_ecran: bool,
    pub touche_quitter: String,
    pub timeout_fichier_secs: u64,
    pub retries_upload: u32,
}

/// Configuration complète de l'application
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub surveillance: SurveillanceConfig,
    pub serveur_ftp: ServeurFtpConfig,
    pub ftp_client: FtpClientConfig,
    pub piia: PiiaConfig,
    pub automate: AutomateConfig,
    pub interface: InterfaceConfig,
}

fn get_str(ini: &Ini, section: &str, key: &str, default: &str) -> String {
    ini.get_from(Some(section), key)
        .unwrap_or(default)
        .trim()
        .to_string()
}

fn get_bool(ini: &Ini, section: &str, key: &str, default: bool) -> bool {
    ini.get_from(Some(section), key)
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

fn get_u16(ini: &Ini, section: &str, key: &str, default: u16) -> u16 {
    ini.get_from(Some(section), key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn get_usize(ini: &Ini, section: &str, key: &str, default: usize) -> usize {
    ini.get_from(Some(section), key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

impl AppConfig {
    pub fn charger(chemin: &Path) -> Result<Self, String> {
        let ini = Ini::load_from_file(chemin)
            .map_err(|e| format!("Impossible de charger {}: {}", chemin.display(), e))?;

        // [surveillance]
        let repertoire = get_str(&ini, "surveillance", "repertoire_surveille", "images_input");
        let extensions_str = get_str(&ini, "surveillance", "extensions", ".jpg,.jpeg,.png");
        let extensions: Vec<String> = extensions_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let dossier_envoye_str = get_str(&ini, "surveillance", "dossier_envoye", "");
        let dossier_envoye = if dossier_envoye_str.is_empty() {
            None
        } else {
            Some(PathBuf::from(dossier_envoye_str))
        };

        // [serveur_ftp]
        let srv_utilisateur = get_str(&ini, "serveur_ftp", "utilisateur", "");
        let srv_mot_de_passe = get_str(&ini, "serveur_ftp", "mot_de_passe", "");
        let masquerade = get_str(&ini, "serveur_ftp", "masquerade_address", "");

        // [ftp]
        let ftp_host = get_str(&ini, "ftp", "host", "192.168.1.100");
        let ftp_utilisateur = get_str(&ini, "ftp", "utilisateur", "ftpuser");
        let ftp_mot_de_passe = get_str(&ini, "ftp", "mot_de_passe", "ftppass");
        let ftp_dossier = get_str(&ini, "ftp", "dossier_distant", "/images");

        Ok(AppConfig {
            surveillance: SurveillanceConfig {
                repertoire_surveille: PathBuf::from(repertoire),
                extensions,
                dossier_envoye,
            },
            serveur_ftp: ServeurFtpConfig {
                actif: get_bool(&ini, "serveur_ftp", "actif", true),
                port_ecoute: get_u16(&ini, "serveur_ftp", "port_ecoute", 2121),
                adresse_ecoute: get_str(&ini, "serveur_ftp", "adresse_ecoute", "0.0.0.0"),
                utilisateur: if srv_utilisateur.is_empty() { None } else { Some(srv_utilisateur) },
                mot_de_passe: if srv_mot_de_passe.is_empty() { None } else { Some(srv_mot_de_passe) },
                anonyme: get_bool(&ini, "serveur_ftp", "anonyme", true),
                masquerade_address: if masquerade.is_empty() { None } else { Some(masquerade) },
                port_passif_min: get_u16(&ini, "serveur_ftp", "port_passif_min", 60000),
                port_passif_max: get_u16(&ini, "serveur_ftp", "port_passif_max", 60100),
            },
            ftp_client: FtpClientConfig {
                host: ftp_host,
                port: get_u16(&ini, "ftp", "port", 21),
                utilisateur: ftp_utilisateur,
                mot_de_passe: ftp_mot_de_passe,
                dossier_distant: ftp_dossier,
                max_images: get_usize(&ini, "ftp", "max_images", 20000),
                mode_passif: get_bool(&ini, "ftp", "mode_passif", true),
            },
            piia: PiiaConfig {
                actif: get_bool(&ini, "piia", "actif", false),
                url: get_str(&ini, "piia", "url", "http://127.0.0.1:3001"),
                timeout_ms: get_usize(&ini, "piia", "timeout_ms", 1000) as u64,
            },
            automate: AutomateConfig {
                actif: get_bool(&ini, "automate", "actif", false),
                driver_type: get_str(&ini, "automate", "driver_type", "none"),
                address: {
                    let s = get_str(&ini, "automate", "address", "");
                    if s.is_empty() { None } else { Some(s) }
                },
                result_tag: {
                    let s = get_str(&ini, "automate", "result_tag", "");
                    if s.is_empty() { None } else { Some(s) }
                },
                heartbeat_tag: {
                    let s = get_str(&ini, "automate", "heartbeat_tag", "");
                    if s.is_empty() { None } else { Some(s) }
                },
                fichier_resultat: {
                    let s = get_str(&ini, "automate", "fichier_resultat", "");
                    if s.is_empty() { None } else { Some(s) }
                },
                tcp_adresse: {
                    let s = get_str(&ini, "automate", "tcp_adresse", "");
                    if s.is_empty() { None } else { Some(s) }
                },
            },
            interface: InterfaceConfig {
                plein_ecran: get_bool(&ini, "interface", "plein_ecran", true),
                touche_quitter: get_str(&ini, "interface", "touche_quitter", "Escape"),
                timeout_fichier_secs: get_usize(&ini, "interface", "timeout_fichier_secs", 15) as u64,
                retries_upload: get_usize(&ini, "interface", "retries_upload", 3) as u32,
            },
        })
    }
}

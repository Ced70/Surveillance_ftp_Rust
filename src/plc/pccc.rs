//! Client PCCC (Programmable Controller Communication Commands) over EtherNet/IP.
//!
//! Version synchrone (std::net::TcpStream) du protocole PCCC pour communiquer
//! avec les automates Allen-Bradley MicroLogix et SLC-500.
//!
//! Compatible avec l'implementation PIIA (meme protocole, memes constantes).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

// === Constantes du protocole (identiques a PIIA) ===

/// Port EtherNet/IP standard
pub const EIP_PORT: u16 = 44818;

/// Taille du header EtherNet/IP
const EIP_HEADER_SIZE: usize = 24;

// Commandes EtherNet/IP
const CMD_REGISTER_SESSION: u16 = 0x0065;
const CMD_UNREGISTER_SESSION: u16 = 0x0066;
const CMD_SEND_RR_DATA: u16 = 0x006F;

// CIP
const CIP_SERVICE_EXECUTE_PCCC: u8 = 0x4B;
const CIP_PCCC_CLASS: u8 = 0x67;

// PCCC
const PCCC_CMD_CODE: u8 = 0x0F;
const PCCC_FNC_WRITE: u8 = 0xAA; // Protected typed logical write, 3 address fields

// Types de fichier PCCC
const PCCC_FILE_TYPE_N: u8 = 0x89; // Integer (INT, 16 bits)
const PCCC_FILE_TYPE_B: u8 = 0x85; // Binary
const PCCC_FILE_TYPE_F: u8 = 0x8A; // Float

/// Adresse d'un fichier de donnees PCCC (ex: N7:0, N150:10)
#[derive(Debug, Clone)]
pub struct PcccAddress {
    pub file_type: u8,
    pub file_number: u8,
    pub element_number: u8,
    pub sub_element: u8,
}

impl PcccAddress {
    /// Parse une adresse au format "N7:0", "N150:10", "B3:0/5", "F8:0"
    pub fn parse(address: &str) -> anyhow::Result<Self> {
        let address = address.trim();
        if address.is_empty() {
            anyhow::bail!("Adresse PCCC vide");
        }

        let file_type_char = address.chars().next().unwrap().to_ascii_uppercase();
        let file_type = match file_type_char {
            'N' => PCCC_FILE_TYPE_N,
            'B' => PCCC_FILE_TYPE_B,
            'F' => PCCC_FILE_TYPE_F,
            _ => anyhow::bail!("Type de fichier '{}' non supporte (N, B, F)", file_type_char),
        };

        let rest = &address[1..];
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "Format d'adresse invalide '{}', attendu: N<file>:<element> (ex: N7:0)",
                address
            );
        }

        let file_number: u8 = parts[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("Numero de fichier invalide: '{}'", parts[0]))?;

        let elem_parts: Vec<&str> = parts[1].split('/').collect();
        let element_number: u8 = elem_parts[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("Numero d'element invalide: '{}'", elem_parts[0]))?;

        let sub_element: u8 = if elem_parts.len() > 1 {
            elem_parts[1]
                .parse()
                .map_err(|_| anyhow::anyhow!("Sous-element invalide: '{}'", elem_parts[1]))?
        } else {
            0
        };

        Ok(Self {
            file_type,
            file_number,
            element_number,
            sub_element,
        })
    }
}

/// Client PCCC over EtherNet/IP (version synchrone)
pub struct PcccClient {
    stream: TcpStream,
    session_handle: u32,
    transaction: AtomicU16,
}

impl PcccClient {
    /// Connecte au PLC et enregistre une session EtherNet/IP
    pub fn connect(address: &str, timeout: Duration) -> anyhow::Result<Self> {
        let addr = if address.contains(':') {
            address.to_string()
        } else {
            format!("{}:{}", address, EIP_PORT)
        };

        let sock_addr: std::net::SocketAddr = addr.parse()
            .map_err(|_| anyhow::anyhow!("Adresse invalide: {}", addr))?;

        let stream = TcpStream::connect_timeout(&sock_addr, timeout)
            .map_err(|e| anyhow::anyhow!("Connexion TCP echouee vers {} : {}", addr, e))?;

        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        let mut client = Self {
            stream,
            session_handle: 0,
            transaction: AtomicU16::new(1),
        };

        client.register_session()?;
        Ok(client)
    }

    /// Enregistre une session EtherNet/IP
    fn register_session(&mut self) -> anyhow::Result<()> {
        let payload = [0x01, 0x00, 0x00, 0x00];
        let packet = self.build_eip_header(CMD_REGISTER_SESSION, 0, &payload);

        self.stream.write_all(&packet)?;

        let response = self.read_eip_response()?;
        if response.len() < EIP_HEADER_SIZE {
            anyhow::bail!("Reponse Register Session trop courte");
        }

        self.session_handle = u32::from_le_bytes([
            response[4], response[5], response[6], response[7],
        ]);

        if self.session_handle == 0 {
            anyhow::bail!("Register Session echoue (session_handle = 0)");
        }

        log::debug!("[PCCC] Session enregistree: 0x{:08X}", self.session_handle);
        Ok(())
    }

    /// Ferme la session et la connexion
    pub fn close(&mut self) {
        let packet = self.build_eip_header(CMD_UNREGISTER_SESSION, self.session_handle, &[]);
        let _ = self.stream.write_all(&packet);
    }

    /// Ecrit un INT (i16) dans un fichier de donnees PCCC
    pub fn write_int(&mut self, address: &PcccAddress, value: i16) -> anyhow::Result<()> {
        let transaction = self.transaction.fetch_add(1, Ordering::SeqCst);

        let mut pccc_msg = Vec::with_capacity(32);

        // CIP header : Execute PCCC service
        pccc_msg.push(CIP_SERVICE_EXECUTE_PCCC);
        pccc_msg.push(0x02);               // Request path size (2 words)
        pccc_msg.push(0x20);               // 8-bit class segment
        pccc_msg.push(CIP_PCCC_CLASS);     // PCCC class (0x67)
        pccc_msg.push(0x24);               // 8-bit instance segment
        pccc_msg.push(0x01);               // Instance 1

        // Requestor ID
        pccc_msg.push(0x07);               // Length
        pccc_msg.extend_from_slice(&[0x09, 0x10]); // Vendor ID
        pccc_msg.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // Serial

        // PCCC command
        pccc_msg.push(PCCC_CMD_CODE);      // 0x0F
        pccc_msg.push(0x00);               // Status
        pccc_msg.extend_from_slice(&transaction.to_le_bytes());

        // Function: Protected Typed Logical Write
        pccc_msg.push(PCCC_FNC_WRITE);     // 0xAA
        pccc_msg.push(0x02);               // Data size (2 bytes = INT)

        // Address: 3 fields
        pccc_msg.push(address.file_number);
        pccc_msg.push(address.file_type);
        pccc_msg.push(address.element_number);
        pccc_msg.push(address.sub_element);

        // Valeur INT (little-endian)
        pccc_msg.extend_from_slice(&value.to_le_bytes());

        // Encapsuler dans Send RR Data
        let rr_data = self.build_send_rr_data(&pccc_msg);
        let packet = self.build_eip_header(CMD_SEND_RR_DATA, self.session_handle, &rr_data);

        self.stream.write_all(&packet)?;

        let response = self.read_eip_response()?;
        self.check_pccc_response(&response)?;

        Ok(())
    }

    // === Helpers ===

    fn build_eip_header(&self, command: u16, session: u32, data: &[u8]) -> Vec<u8> {
        let mut header = Vec::with_capacity(EIP_HEADER_SIZE + data.len());
        header.extend_from_slice(&command.to_le_bytes());
        header.extend_from_slice(&(data.len() as u16).to_le_bytes());
        header.extend_from_slice(&session.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(b"_surv___");  // Sender Context
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(data);
        header
    }

    fn build_send_rr_data(&self, cip_message: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(16 + cip_message.len());
        data.extend_from_slice(&0u32.to_le_bytes());           // Interface Handle
        data.extend_from_slice(&10u16.to_le_bytes());          // Timeout
        data.extend_from_slice(&2u16.to_le_bytes());           // Item Count

        // Item 1: Null Address
        data.extend_from_slice(&0x0000u16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());

        // Item 2: Unconnected Data Item
        data.extend_from_slice(&0x00B2u16.to_le_bytes());
        data.extend_from_slice(&(cip_message.len() as u16).to_le_bytes());
        data.extend_from_slice(cip_message);

        data
    }

    fn read_eip_response(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut header = [0u8; EIP_HEADER_SIZE];
        self.stream.read_exact(&mut header)?;

        let data_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        let status = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);

        if status != 0 {
            anyhow::bail!("Erreur EtherNet/IP: status = 0x{:08X}", status);
        }

        let mut response = Vec::with_capacity(EIP_HEADER_SIZE + data_len);
        response.extend_from_slice(&header);

        if data_len > 0 {
            let mut data = vec![0u8; data_len];
            self.stream.read_exact(&mut data)?;
            response.extend_from_slice(&data);
        }

        Ok(response)
    }

    fn check_pccc_response(&self, response: &[u8]) -> anyhow::Result<()> {
        if response.len() < 42 {
            anyhow::bail!("Reponse trop courte ({} bytes)", response.len());
        }

        let service_reply = response[40];
        if service_reply & 0x80 == 0 {
            anyhow::bail!("Reponse CIP invalide: 0x{:02X}", service_reply);
        }

        let cip_status = response[42];
        if cip_status != 0 {
            anyhow::bail!("Erreur CIP: status = 0x{:02X}", cip_status);
        }

        let pccc_sts_offset = 40 + 4 + 7 + 1; // 52
        if response.len() > pccc_sts_offset {
            let pccc_status = response[pccc_sts_offset];
            if pccc_status != 0 {
                anyhow::bail!(
                    "Erreur PCCC: status = 0x{:02X} (commande refusee par l'automate)",
                    pccc_status
                );
            }
        }

        Ok(())
    }
}

impl Drop for PcccClient {
    fn drop(&mut self) {
        self.close();
    }
}

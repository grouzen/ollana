use std::{
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use log::{debug, info};
use rustls::pki_types::{pem::PemObject, PrivatePkcs8KeyDer};

const CLIENT_CERT_PEM: &str = "client_cert.pem";
const CLIENT_KEY_PEM: &str = "client_key.pem";
const SERVER_CERT_PEM: &str = "server_cert.pem";
const SERVER_KEY_PEM: &str = "server_key.pem";
const HTTP_SERVER_CERT_PEM: &str = "http_server_cert.pem";
const HTTP_SERVER_KEY_PEM: &str = "http_server_key.pem";

pub struct Certs {
    dir: PathBuf,
}

impl Certs {
    pub fn new() -> anyhow::Result<Self> {
        let dir = dirs::data_local_dir()
            .map(|p| p.join("ollana"))
            .ok_or(anyhow::Error::msg(
                "Couldn't determine data local directory",
            ))?;

        Ok(Self { dir })
    }

    /// Generates a client certificate and key.
    ///
    /// This function creates a client certificate and signing key, storing them in files named
    /// "client_cert.pem" and "client_key.pem" respectively within the directory specified by `self.dir`.
    ///
    pub fn gen_client(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join(CLIENT_CERT_PEM);
        let signing_key_path = self.dir.join(CLIENT_KEY_PEM);

        self.gen_x509(&cert_path, &signing_key_path)
    }

    pub fn get_client_device_id(&self) -> anyhow::Result<String> {
        self.get_device_id(CLIENT_KEY_PEM)
    }

    /// Generates a server certificate and key.
    ///
    /// This function creates a server certificate and signing key, storing them in files named
    /// "server_cert.pem" and "server_key.pem" respectively within the directory specified by `self.dir`.
    ///
    pub fn gen_server(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join(SERVER_CERT_PEM);
        let signing_key_path = self.dir.join(SERVER_KEY_PEM);

        self.gen_x509(&cert_path, &signing_key_path)
    }

    pub fn get_server_device_id(&self) -> anyhow::Result<String> {
        self.get_device_id(SERVER_KEY_PEM)
    }

    /// Generates an HTTP server certificate and key.
    ///
    /// This function creates a HTTP server certificate and signing key, storing them in files named
    /// "http_server_cert.pem" and "http_server_key.pem" respectively within the directory specified by `self.dir`.
    ///
    pub fn gen_http_server(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join(HTTP_SERVER_CERT_PEM);
        let signing_key_path = self.dir.join(HTTP_SERVER_KEY_PEM);

        self.gen_x509(&cert_path, &signing_key_path)
    }

    /// Gets the server's certificate and private key files.
    ///
    /// This function retrieves the PEM-encoded X.509 certificate file and the corresponding RSA or ECDSA
    /// private key file used for generating a device fingerpint by the server.
    ///
    pub fn get_server_files(&self) -> anyhow::Result<(File, File)> {
        let cert_file = File::open(self.dir.join(SERVER_CERT_PEM))?;
        let signing_key_file = File::open(self.dir.join(SERVER_KEY_PEM))?;

        Ok((cert_file, signing_key_file))
    }

    /// Gets the HTTP server's certificate and private key files.
    ///
    /// This function retrieves the PEM-encoded X.509 certificate file and the corresponding RSA or ECDSA
    /// private key file used for HTTPS communication by the server.
    ///
    pub fn get_http_server_files(&self) -> anyhow::Result<(File, File)> {
        let cert_file = File::open(self.dir.join(HTTP_SERVER_CERT_PEM))?;
        let signing_key_file = File::open(self.dir.join(HTTP_SERVER_KEY_PEM))?;

        Ok((cert_file, signing_key_file))
    }

    /// Generates an X.509 certificate and signing key if they do not already exist.
    ///
    /// # Arguments
    ///
    /// * `cert_path` - A reference to the path where the generated X.509 certificate will be saved.
    /// * `signing_key_path` - A reference to the path where the generated signing key will be saved.
    ///
    fn gen_x509(&self, cert_path: &Path, signing_key_path: &Path) -> anyhow::Result<()> {
        if !self.dir.exists() {
            debug!(
                "Creating data local dir to store certificates: {}",
                self.dir.as_path().to_string_lossy()
            );

            std::fs::create_dir_all(self.dir.as_path())?;
        }

        if !(cert_path.exists() && signing_key_path.exists()) {
            info!(
                "Couldn't find an already existing X509 pem files, generating new: {}, {}",
                cert_path.to_string_lossy(),
                signing_key_path.to_string_lossy()
            );

            let cert = rcgen::generate_simple_self_signed(vec!["*".into()])?;

            let cert_pem = cert.cert.pem();
            let signing_key_pem = cert.signing_key.serialize_pem();

            std::fs::write(cert_path, cert_pem)?;
            std::fs::write(signing_key_path, signing_key_pem)?;
            std::fs::set_permissions(cert_path, std::fs::Permissions::from_mode(0o400u32))?;
            std::fs::set_permissions(signing_key_path, std::fs::Permissions::from_mode(0o400u32))?;
        }

        Ok(())
    }

    fn get_device_id(&self, key_file_path: &str) -> anyhow::Result<String> {
        let signing_key_path = self.dir.join(key_file_path);
        let der = PrivatePkcs8KeyDer::from_pem_file(signing_key_path)?;

        Ok(sha256::digest(der.secret_pkcs8_der()))
    }
}

use std::{
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use log::{debug, info};

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
        let cert_path = self.dir.join("client_cert.pem");
        let signing_key_path = self.dir.join("client_key.pem");

        self.gen_x509(&cert_path, &signing_key_path)
    }

    /// Generates a server certificate and key.
    ///
    /// This function creates a server certificate and signing key, storing them in files named
    /// "server_cert.pem" and "server_key.pem" respectively within the directory specified by `self.dir`.
    ///
    pub fn gen_server(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join("server_cert.pem");
        let signing_key_path = self.dir.join("server_key.pem");

        self.gen_x509(&cert_path, &signing_key_path)
    }

    pub fn get_server_files(&self) -> anyhow::Result<(File, File)> {
        let cert_file = File::open(self.dir.join("server_cert.pem"))?;
        let signing_key_file = File::open(self.dir.join("server_key.pem"))?;

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
}

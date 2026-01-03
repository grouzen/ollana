use std::{
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use log::{debug, info};
use rustls::pki_types::{pem::PemObject, PrivatePkcs8KeyDer};

use crate::get_local_dir;

const DEVICE_CERT_PEM: &str = "device_cert.pem";
const DEVICE_KEY_PEM: &str = "device_key.pem";
const HTTP_SERVER_CERT_PEM: &str = "http_server_cert.pem";
const HTTP_SERVER_KEY_PEM: &str = "http_server_key.pem";

/// Trait for certificate management operations.
/// This allows for different implementations of certificate generation and retrieval.
pub trait Certs: Send + Sync {
    /// Generates a device certificate and key.
    ///
    /// This function creates a device certificate and signing key, storing them in files named
    /// "device_cert.pem" and "device_key.pem" respectively within the directory specified by `self.dir`.
    ///
    fn gen_device(&self) -> anyhow::Result<()>;

    /// Retrieves the device key bytes from the PEM file.
    ///
    /// This function reads the private PKCS#8 DER-encoded secret key from the PEM file located at `DEVICE_KEY_PEM`
    /// within the configured directory. It returns a vector containing the DER-encoded key bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if there is any issue reading or parsing the PEM file.
    fn get_device_key_bytes(&self) -> anyhow::Result<Vec<u8>>;

    /// Generates an HTTP server certificate and key.
    ///
    /// This function creates a HTTP server certificate and signing key, storing them in files named
    /// "http_server_cert.pem" and "http_server_key.pem" respectively within the directory specified by `self.dir`.
    ///
    fn gen_http_server(&self) -> anyhow::Result<()>;

    /// Gets the HTTP server's certificate and private key files.
    ///
    /// This function retrieves the PEM-encoded X.509 certificate file and the corresponding RSA or ECDSA
    /// private key file used for HTTPS communication by the server.
    ///
    fn get_http_server_files(&self) -> anyhow::Result<(File, File)>;
}

/// X.509 certificate implementation.
pub struct X509Certs {
    dir: PathBuf,
}

impl X509Certs {
    pub fn new() -> anyhow::Result<Self> {
        let dir = get_local_dir()?;

        Ok(Self { dir })
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

impl Certs for X509Certs {
    fn gen_device(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join(DEVICE_CERT_PEM);
        let signing_key_path = self.dir.join(DEVICE_KEY_PEM);

        self.gen_x509(&cert_path, &signing_key_path)
    }

    fn get_device_key_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let signing_key_path = self.dir.join(DEVICE_KEY_PEM);
        let der = PrivatePkcs8KeyDer::from_pem_file(signing_key_path)?;

        Ok(der.secret_pkcs8_der().to_vec())
    }

    fn gen_http_server(&self) -> anyhow::Result<()> {
        let cert_path = self.dir.join(HTTP_SERVER_CERT_PEM);
        let signing_key_path = self.dir.join(HTTP_SERVER_KEY_PEM);

        self.gen_x509(&cert_path, &signing_key_path)
    }

    fn get_http_server_files(&self) -> anyhow::Result<(File, File)> {
        let cert_file = File::open(self.dir.join(HTTP_SERVER_CERT_PEM))?;
        let signing_key_file = File::open(self.dir.join(HTTP_SERVER_KEY_PEM))?;

        Ok((cert_file, signing_key_file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create X509Certs with a temporary directory for testing
    fn create_test_certs() -> (X509Certs, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let certs = X509Certs {
            dir: temp_dir.path().to_path_buf(),
        };
        (certs, temp_dir)
    }

    #[test]
    fn test_gen_device_creates_certificate_files() {
        let (certs, _temp_dir) = create_test_certs();

        let result = certs.gen_device();
        assert!(result.is_ok(), "gen_device should succeed");

        let cert_path = certs.dir.join(DEVICE_CERT_PEM);
        let key_path = certs.dir.join(DEVICE_KEY_PEM);

        assert!(cert_path.exists(), "Device certificate file should exist");
        assert!(key_path.exists(), "Device key file should exist");
    }

    #[test]
    fn test_gen_device_idempotent() {
        let (certs, _temp_dir) = create_test_certs();

        // Generate certificates first time
        certs.gen_device().unwrap();

        let cert_path = certs.dir.join(DEVICE_CERT_PEM);
        let key_path = certs.dir.join(DEVICE_KEY_PEM);

        // Get file metadata to check if files are overwritten
        let cert_metadata1 = fs::metadata(&cert_path).unwrap();
        let key_metadata1 = fs::metadata(&key_path).unwrap();

        // Generate certificates second time
        certs.gen_device().unwrap();

        let cert_metadata2 = fs::metadata(&cert_path).unwrap();
        let key_metadata2 = fs::metadata(&key_path).unwrap();

        // Files should not be overwritten (same modification time)
        assert_eq!(
            cert_metadata1.modified().unwrap(),
            cert_metadata2.modified().unwrap(),
            "Certificate file should not be regenerated"
        );
        assert_eq!(
            key_metadata1.modified().unwrap(),
            key_metadata2.modified().unwrap(),
            "Key file should not be regenerated"
        );
    }

    #[test]
    fn test_get_device_key_bytes() {
        let (certs, _temp_dir) = create_test_certs();

        // Generate device certificate first
        certs.gen_device().unwrap();

        // Get key bytes
        let result = certs.get_device_key_bytes();
        assert!(result.is_ok(), "get_device_key_bytes should succeed");

        let key_bytes = result.unwrap();
        assert!(!key_bytes.is_empty(), "Key bytes should not be empty");
    }

    #[test]
    fn test_get_device_key_bytes_fails_without_generation() {
        let (certs, _temp_dir) = create_test_certs();

        // Try to get key bytes without generating certificates first
        let result = certs.get_device_key_bytes();
        assert!(
            result.is_err(),
            "get_device_key_bytes should fail when certificates don't exist"
        );
    }

    #[test]
    fn test_gen_http_server_creates_certificate_files() {
        let (certs, _temp_dir) = create_test_certs();

        let result = certs.gen_http_server();
        assert!(result.is_ok(), "gen_http_server should succeed");

        let cert_path = certs.dir.join(HTTP_SERVER_CERT_PEM);
        let key_path = certs.dir.join(HTTP_SERVER_KEY_PEM);

        assert!(
            cert_path.exists(),
            "HTTP server certificate file should exist"
        );
        assert!(key_path.exists(), "HTTP server key file should exist");
    }

    #[test]
    fn test_gen_http_server_idempotent() {
        let (certs, _temp_dir) = create_test_certs();

        // Generate certificates first time
        certs.gen_http_server().unwrap();

        let cert_path = certs.dir.join(HTTP_SERVER_CERT_PEM);
        let key_path = certs.dir.join(HTTP_SERVER_KEY_PEM);

        // Get file metadata to check if files are overwritten
        let cert_metadata1 = fs::metadata(&cert_path).unwrap();
        let key_metadata1 = fs::metadata(&key_path).unwrap();

        // Generate certificates second time
        certs.gen_http_server().unwrap();

        let cert_metadata2 = fs::metadata(&cert_path).unwrap();
        let key_metadata2 = fs::metadata(&key_path).unwrap();

        // Files should not be overwritten (same modification time)
        assert_eq!(
            cert_metadata1.modified().unwrap(),
            cert_metadata2.modified().unwrap(),
            "Certificate file should not be regenerated"
        );
        assert_eq!(
            key_metadata1.modified().unwrap(),
            key_metadata2.modified().unwrap(),
            "Key file should not be regenerated"
        );
    }

    #[test]
    fn test_get_http_server_files() {
        let (certs, _temp_dir) = create_test_certs();

        // Generate HTTP server certificates first
        certs.gen_http_server().unwrap();

        // Get certificate and key files
        let result = certs.get_http_server_files();
        assert!(result.is_ok(), "get_http_server_files should succeed");

        let (cert_file, key_file) = result.unwrap();

        // Verify we can read from the files (they're valid File handles)
        assert!(
            cert_file.metadata().is_ok(),
            "Certificate file should be readable"
        );
        assert!(key_file.metadata().is_ok(), "Key file should be readable");
    }

    #[test]
    fn test_get_http_server_files_fails_without_generation() {
        let (certs, _temp_dir) = create_test_certs();

        // Try to get files without generating certificates first
        let result = certs.get_http_server_files();
        assert!(
            result.is_err(),
            "get_http_server_files should fail when certificates don't exist"
        );
    }

    #[test]
    fn test_certificate_file_permissions() {
        let (certs, _temp_dir) = create_test_certs();

        certs.gen_device().unwrap();

        let cert_path = certs.dir.join(DEVICE_CERT_PEM);
        let key_path = certs.dir.join(DEVICE_KEY_PEM);

        let cert_metadata = fs::metadata(&cert_path).unwrap();
        let key_metadata = fs::metadata(&key_path).unwrap();

        // Check that permissions are set to 0o400 (read-only for owner)
        assert_eq!(
            cert_metadata.permissions().mode() & 0o777,
            0o400,
            "Certificate file should have 0o400 permissions"
        );
        assert_eq!(
            key_metadata.permissions().mode() & 0o777,
            0o400,
            "Key file should have 0o400 permissions"
        );
    }

    #[test]
    fn test_gen_x509_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let nested_dir = temp_dir.path().join("nested").join("dir");

        let certs = X509Certs {
            dir: nested_dir.clone(),
        };

        assert!(
            !nested_dir.exists(),
            "Nested directory should not exist yet"
        );

        certs.gen_device().unwrap();

        assert!(nested_dir.exists(), "Nested directory should be created");
    }

    #[test]
    fn test_device_and_http_certificates_are_independent() {
        let (certs, _temp_dir) = create_test_certs();

        // Generate both types of certificates
        certs.gen_device().unwrap();
        certs.gen_http_server().unwrap();

        // Verify all four files exist
        let device_cert_path = certs.dir.join(DEVICE_CERT_PEM);
        let device_key_path = certs.dir.join(DEVICE_KEY_PEM);
        let http_cert_path = certs.dir.join(HTTP_SERVER_CERT_PEM);
        let http_key_path = certs.dir.join(HTTP_SERVER_KEY_PEM);

        assert!(device_cert_path.exists());
        assert!(device_key_path.exists());
        assert!(http_cert_path.exists());
        assert!(http_key_path.exists());

        // Verify they have different content
        let device_cert = fs::read_to_string(&device_cert_path).unwrap();
        let http_cert = fs::read_to_string(&http_cert_path).unwrap();

        assert_ne!(
            device_cert, http_cert,
            "Device and HTTP certificates should be different"
        );
    }
}

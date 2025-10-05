use crate::certs::Certs;

pub struct Device {
    pub id: String,
}

impl Device {
    pub fn new(certs: &Certs) -> anyhow::Result<Self> {
        certs.gen_device()?;

        let id = sha256::digest(certs.get_device_key_bytes()?);

        Ok(Self { id })
    }
}

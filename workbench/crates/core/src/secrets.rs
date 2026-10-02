//! Provider credentials. Stored in the OS keychain; falls back to an
//! owner-only file when no Secret Service is running (common on bare Linux WMs).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

const SERVICE: &str = "workbench";

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Credential {
    ApiKey {
        key: String,
    },
    #[serde(rename_all = "camelCase")]
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        /// Unix seconds.
        expires_at: Option<i64>,
        client_id: Option<String>,
        account: Option<String>,
    },
}

impl Credential {
    pub fn bearer(&self) -> &str {
        match self {
            Self::ApiKey { key } => key,
            Self::OAuth { access_token, .. } => access_token,
        }
    }

    pub fn needs_refresh(&self) -> bool {
        match self {
            Self::OAuth {
                expires_at: Some(at),
                refresh_token: Some(_),
                ..
            } => *at - crate::now_secs() < 120,
            _ => false,
        }
    }
}

enum Backend {
    Keyring,
    File(PathBuf),
}

pub struct Vault {
    backend: Backend,
    cache: Mutex<HashMap<ProviderId, Option<Credential>>>,
}

impl Vault {
    pub fn open(fallback_file: PathBuf) -> Self {
        let keyring_ok = keyring::Entry::new(SERVICE, "probe")
            .and_then(|e| match e.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err),
            })
            .is_ok();
        let backend = if keyring_ok {
            Backend::Keyring
        } else {
            Backend::File(fallback_file)
        };
        Self {
            backend,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Keyring => "keychain",
            Backend::File(_) => "file",
        }
    }

    pub fn get(&self, provider: ProviderId) -> anyhow::Result<Option<Credential>> {
        if let Some(hit) = self.cache.lock().unwrap().get(&provider) {
            return Ok(hit.clone());
        }
        let loaded = match &self.backend {
            Backend::Keyring => match entry(provider)?.get_password() {
                Ok(json) => {
                    Some(serde_json::from_str(&json).context("stored credential is corrupt")?)
                }
                Err(keyring::Error::NoEntry) => None,
                Err(e) => return Err(e).context("reading the keychain"),
            },
            Backend::File(path) => read_file(path)?.remove(provider.key()),
        };
        self.cache.lock().unwrap().insert(provider, loaded.clone());
        Ok(loaded)
    }

    pub fn set(&self, provider: ProviderId, credential: &Credential) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Keyring => entry(provider)?
                .set_password(&serde_json::to_string(credential)?)
                .context("writing the keychain")?,
            Backend::File(path) => {
                let mut all = read_file(path)?;
                all.insert(provider.key().to_owned(), credential.clone());
                write_file(path, &all)?;
            }
        }
        self.cache
            .lock()
            .unwrap()
            .insert(provider, Some(credential.clone()));
        Ok(())
    }

    pub fn remove(&self, provider: ProviderId) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Keyring => match entry(provider)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(e).context("deleting from the keychain"),
            },
            Backend::File(path) => {
                let mut all = read_file(path)?;
                all.remove(provider.key());
                write_file(path, &all)?;
            }
        }
        self.cache.lock().unwrap().insert(provider, None);
        Ok(())
    }
}

fn entry(provider: ProviderId) -> anyhow::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, provider.key()).context("opening the keychain")
}

fn read_file(path: &PathBuf) -> anyhow::Result<BTreeMap<String, Credential>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_file(path: &PathBuf, all: &BTreeMap<String, Credential>) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(all)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        std::fs::rename(tmp, path)?;
    }
    #[cfg(not(unix))]
    crate::write_atomic(path, &bytes)?;
    Ok(())
}

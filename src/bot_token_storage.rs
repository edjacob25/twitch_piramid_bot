use async_trait::async_trait;
use config::Config;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use twitch_irc::login::{TokenStorage, UserAccessToken};

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Deserialize(config::ConfigError),
}

impl Display for LoadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write! {f, "IO error: {} ", e},
            LoadError::Deserialize(e) => write! {f, "Deserializing error: {} ", e},
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LoadError::Io(e) => Some(e),
            LoadError::Deserialize(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<config::ConfigError> for LoadError {
    fn from(value: config::ConfigError) -> Self {
        Self::Deserialize(value)
    }
}

#[derive(Debug)]
pub enum UpdateError {
    Io(std::io::Error),
    Parsing(toml::ser::Error),
}

impl Display for UpdateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Io(e) => write! {f, "IO error: {} ", e},
            UpdateError::Parsing(e) => write! {f, "Parsing error: {} ", e},
        }
    }
}

impl Error for UpdateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            UpdateError::Io(e) => Some(e),
            UpdateError::Parsing(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::ser::Error> for UpdateError {
    fn from(value: toml::ser::Error) -> Self {
        Self::Parsing(value)
    }
}

#[derive(Clone, Debug)]
pub struct CustomTokenStorage {
    pub location: String,
}

#[async_trait]
impl TokenStorage for CustomTokenStorage {
    type LoadError = LoadError;
    type UpdateError = UpdateError;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        let file = Config::builder()
            .add_source(config::File::with_name(&self.location))
            .build();

        Ok(file?.try_deserialize::<UserAccessToken>()?)
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        // Called after the token was updated successfully, to save the new token.
        // After `update_token()` completes, the `load_token()` method should then return
        // that token for future invocations
        let p = Path::new(&self.location);
        let toml_string = toml::to_string(token)?;
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            env::current_dir()?.as_path().join(p)
        };
        File::create(p)?.write_all(toml_string.as_ref())?;
        Ok(())
    }
}

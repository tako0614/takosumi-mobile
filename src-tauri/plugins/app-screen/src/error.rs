use serde::{ser::Serializer, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidRequest(&'static str),
    #[cfg(all(desktop, feature = "desktop"))]
    #[error("{0}")]
    Desktop(String),
    #[cfg(all(desktop, feature = "desktop-process-experiment"))]
    #[error(
        "The browser ended abnormally. Saved data is retained; explicit recovery is available."
    )]
    BrowserRecoveryRequired {
        profile: String,
        key: String,
        generation: String,
    },
    #[cfg(mobile)]
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[cfg(all(desktop, feature = "desktop-process-experiment"))]
        if let Self::BrowserRecoveryRequired {
            profile,
            key,
            generation,
        } = self
        {
            #[derive(Serialize)]
            struct Recovery<'a> {
                kind: &'static str,
                profile: &'a str,
                key: &'a str,
                generation: &'a str,
            }
            return Recovery {
                kind: "browserProfileRecoveryRequired",
                profile,
                key,
                generation,
            }
            .serialize(serializer);
        }
        serializer.serialize_str(self.to_string().as_ref())
    }
}

//! DLightroom's network clients, all behind explicit user actions: provider-based AI image
//! variations ([`ai`]), OAuth 2.0 for installed apps ([`oauth`]), Google Drive uploads
//! ([`drive`]), secrets in the system keychain ([`secrets`]) and background jobs with progress and
//! cancelling ([`job`]). Nothing here ships a credential: the user brings their own API key or
//! OAuth client. Requests go through [`http::Transport`], so every client is tested offline.

pub mod ai;
pub mod drive;
pub mod http;
pub mod job;
pub mod oauth;
pub mod secrets;
pub mod util;

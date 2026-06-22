//! Security module for DM pairing, device identity, and access control

pub mod auth;
pub mod device;
pub mod identity;
pub mod moderation;
pub mod pairing;

pub use auth::{AuthChallenge, AuthConfig, AuthMode, PairingRequest};
pub use device::{DeviceManager, PairedDevice, TrustLevel};
pub use identity::{DeviceIdentity, verify_signature};
pub use pairing::{DmPolicy, PairedUser, PairingManager};

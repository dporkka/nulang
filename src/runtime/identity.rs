use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use std::hash::{Hash, Hasher};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActorId(pub Vec<u8>);

impl ActorId {
    pub fn new() -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let pub_bytes = verifying_key.as_bytes();
        
        let hash = blake3::hash(pub_bytes);
        let mut envelope = Vec::new();
        // multihash prefix for blake3 (0x1e)
        envelope.push(0x1e);
        envelope.push(32); // length
        envelope.extend_from_slice(hash.as_bytes());
        
        ActorId(envelope)
    }
    
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActorId({})", hex::encode(&self.0))
    }
}

impl std::fmt::Display for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

impl Hash for ActorId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}


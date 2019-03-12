use crate::{
    key_bundle::KeyBundle, password_encryption::*, AGENT_ID_CTX_STR, CONTEXT_SIZE, SEED_SIZE,
};
use bip39::{Language, Mnemonic};
use holochain_core_types::error::{HcResult, HolochainError};
use holochain_sodium::{kdf, pwhash, secbuf::SecBuf};
use serde_derive::{Deserialize, Serialize};
use std::str;

//--------------------------------------------------------------------------------------------------
// SeedInitializer
//--------------------------------------------------------------------------------------------------

/// Enum of all possible ways to initialize a Seed
pub enum SeedInitializer {
    Seed(SecBuf),
    Mnemonic(String),
}

//--------------------------------------------------------------------------------------------------
// Seed Types
//--------------------------------------------------------------------------------------------------

/// Enum of all the types of seeds
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SeedType {
    /// Root / Master seed
    Root,
    /// Revocation seed
    Revocation,
    /// Indexed seed
    Indexed,
    /// Derivative of a Indexed seed with a PIN
    IndexedPin,
    /// Application specific seed
    Application,
    /// Seed for a one use only key
    OneShot,
    /// Seed used only in tests or mocks
    Mock,
}

pub struct SeedContext {
    inner: String,
}

impl SeedContext {
    pub fn new(data: String) -> Self {
        assert_eq!(data.len(), CONTEXT_SIZE);
        assert!(data.is_ascii());
        SeedContext { inner: data }
    }

    pub fn to_sec_buf(&self) -> SecBuf {
        SecBuf::with_insecure_from_string(self.inner.clone())
    }
}

impl From<&str> for SeedContext {
    fn from(data: &str) -> SeedContext {
        SeedContext::new(data.to_string())
    }
}

/// Enum of all the different behaviors a Seed can have
pub enum TypedSeed {
    Root(RootSeed),
    Indexed(IndexedSeed),
    IndexedPin(IndexedPinSeed),
}

/// Common Trait for TypedSeeds
pub trait SeedTrait {
    fn seed(&self) -> &Seed;
    fn seed_mut(&mut self) -> &mut Seed;
}

//--------------------------------------------------------------------------------------------------
// Seed
//--------------------------------------------------------------------------------------------------

// Data of a seed
#[derive(Debug)]
pub struct Seed {
    pub kind: SeedType,
    pub buf: SecBuf,
}

impl Seed {
    pub fn new(seed_buf: SecBuf, seed_type: SeedType) -> Self {
        assert_eq!(seed_buf.len(), SEED_SIZE);
        Seed {
            kind: seed_type,
            buf: seed_buf,
        }
    }

    // TODO: We need some way of zeroing the internal memory used by mnemonic
    pub fn new_with_mnemonic(phrase: String, seed_type: SeedType) -> HcResult<Self> {
        let maybe_mnemonic = Mnemonic::from_phrase(phrase, Language::English);
        if let Err(e) = maybe_mnemonic {
            return Err(HolochainError::ErrorGeneric(format!(
                "Error loading Mnemonic phrase: {}",
                e
            )));
        }
        let mnemonic = maybe_mnemonic.unwrap();
        let entropy = mnemonic.entropy().clone();
        assert_eq!(entropy.len(), SEED_SIZE);
        let mut seed_buf = SecBuf::with_secure(entropy.len());
        seed_buf.from_array(entropy)?;
        // Done
        Ok(Seed {
            kind: seed_type,
            buf: seed_buf,
        })
    }

    ///  Construct this seed struct from a SeedInitializer
    ///  @param {string} seed_type -
    ///  @param {SecBuf|string} initializer - data (buffer or mnemonic) for constructing the Seed
    pub fn new_with_initializer(initializer: SeedInitializer, seed_type: SeedType) -> Self {
        match initializer {
            SeedInitializer::Seed(seed_buf) => Seed::new(seed_buf, seed_type),
            SeedInitializer::Mnemonic(phrase) => Seed::new_with_mnemonic(phrase, seed_type)
                .expect("Invalid Mnemonic Seed initializer"),
        }
    }

    pub fn into_typed(self) -> HcResult<TypedSeed> {
        match self.kind {
            SeedType::Root => Ok(TypedSeed::Root(RootSeed::new(self.buf))),
            SeedType::Indexed => Ok(TypedSeed::Indexed(IndexedSeed::new(self.buf))),
            SeedType::IndexedPin => Ok(TypedSeed::IndexedPin(IndexedPinSeed::new(self.buf))),
            _ => Err(HolochainError::ErrorGeneric(
                "Seed does have specific behavior for its type".to_string(),
            )),
        }
    }

    /// Generate a mnemonic for the seed.
    // TODO: We need some way of zeroing the internal memory used by mnemonic
    pub fn get_mnemonic(&mut self) -> HcResult<String> {
        let entropy = self.buf.read_lock();
        let e = &*entropy;
        let mnemonic = Mnemonic::from_entropy(e, Language::English).map_err(|e| {
            HolochainError::ErrorGeneric(format!("Error generating Mnemonic phrase: {}", e))
        })?;
        Ok(mnemonic.into_phrase())
    }
}

//--------------------------------------------------------------------------------------------------
// RootSeed
//--------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct RootSeed {
    inner: Seed,
}

impl SeedTrait for RootSeed {
    fn seed(&self) -> &Seed {
        &self.inner
    }
    fn seed_mut(&mut self) -> &mut Seed {
        &mut self.inner
    }
}

impl RootSeed {
    /// Construct from a 32 bytes seed buffer
    pub fn new(seed_buf: SecBuf) -> Self {
        RootSeed {
            inner: Seed::new_with_initializer(SeedInitializer::Seed(seed_buf), SeedType::Root),
        }
    }

    /// Generate Indexed Seed
    /// @param {number} index - the index number in this seed group, must not be zero
    pub fn generate_indexed_seed(
        &mut self,
        seed_context: &SeedContext,
        index: u64,
    ) -> HcResult<IndexedSeed> {
        if index == 0 {
            return Err(HolochainError::ErrorGeneric("Invalid index".to_string()));
        }
        let mut indexed_seed_buf = SecBuf::with_secure(SEED_SIZE);
        let mut context = seed_context.to_sec_buf();
        kdf::derive(
            &mut indexed_seed_buf,
            index,
            &mut context,
            &mut self.inner.buf,
        )?;
        Ok(IndexedSeed::new(indexed_seed_buf))
    }
}

//--------------------------------------------------------------------------------------------------
// IndexedSeed
//--------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct IndexedSeed {
    inner: Seed,
}

impl SeedTrait for IndexedSeed {
    fn seed(&self) -> &Seed {
        &self.inner
    }
    fn seed_mut(&mut self) -> &mut Seed {
        &mut self.inner
    }
}

impl IndexedSeed {
    /// Construct from a 32 bytes seed buffer
    pub fn new(seed_buf: SecBuf) -> Self {
        IndexedSeed {
            inner: Seed::new_with_initializer(SeedInitializer::Seed(seed_buf), SeedType::Indexed),
        }
    }

    /// generate a indexed pin seed by applying pwhash of pin with this seed as the salt
    /// @param {string} pin - should be >= 4 characters 1-9
    /// @return {IndexedPinSeed} Resulting Indexed Pin Seed
    pub fn generate_indexed_pin_seed(
        &mut self,
        pin: &mut SecBuf,
        config: Option<PwHashConfig>,
    ) -> HcResult<IndexedPinSeed> {
        let mut hash = SecBuf::with_secure(pwhash::HASHBYTES);
        pw_hash(pin, &mut self.inner.buf, &mut hash, config)?;
        Ok(IndexedPinSeed::new(hash))
    }
}

//--------------------------------------------------------------------------------------------------
// IndexedPinSeed
//--------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct IndexedPinSeed {
    inner: Seed,
}

impl SeedTrait for IndexedPinSeed {
    fn seed(&self) -> &Seed {
        &self.inner
    }
    fn seed_mut(&mut self) -> &mut Seed {
        &mut self.inner
    }
}

impl IndexedPinSeed {
    /// Construct from a 32 bytes seed buffer
    pub fn new(seed_buf: SecBuf) -> Self {
        IndexedPinSeed {
            inner: Seed::new_with_initializer(
                SeedInitializer::Seed(seed_buf),
                SeedType::IndexedPin,
            ),
        }
    }

    /// generate an application KeyBundle given an index based on this seed
    /// @param {number} index - must not be zero
    /// @return {KeyBundle} Resulting keybundle
    pub fn generate_application_key(&mut self, index: u64) -> HcResult<KeyBundle> {
        if index == 0 {
            return Err(HolochainError::ErrorGeneric("Invalid index".to_string()));
        }
        let mut app_seed_buf = SecBuf::with_secure(SEED_SIZE);
        let context = SeedContext::from(AGENT_ID_CTX_STR);
        let mut context = context.to_sec_buf();
        kdf::derive(&mut app_seed_buf, index, &mut context, &mut self.inner.buf)?;

        Ok(KeyBundle::new_from_seed_buf(
            &mut app_seed_buf,
            SeedType::Application,
        )?)
    }
}

/// returns a random seed buf
pub fn generate_random_seed_buf(size: usize) -> SecBuf {
    let mut seed = SecBuf::with_insecure(size);
    seed.randomize();
    seed
}

//--------------------------------------------------------------------------------------------------
// Tests
//--------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{password_encryption::tests::TEST_CONFIG, SEED_SIZE};

    #[test]
    fn it_should_create_a_new_seed() {
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let seed_type = SeedType::OneShot;
        let seed = Seed::new_with_initializer(SeedInitializer::Seed(seed_buf), seed_type.clone());
        assert_eq!(seed_type, seed.kind);
    }

    #[test]
    fn it_should_create_a_new_root_seed() {
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let root_seed = RootSeed::new(seed_buf);
        assert_eq!(SeedType::Root, root_seed.seed().kind);
    }

    #[test]
    fn it_should_create_a_indexed_seed() {
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let context = SeedContext::from("HCDEVICE");
        let mut root_seed = RootSeed::new(seed_buf);

        let mut indexed_seed_3 = root_seed.generate_indexed_seed(&context, 3).unwrap();
        assert_eq!(SeedType::Indexed, indexed_seed_3.seed().kind);
        let _ = root_seed.generate_indexed_seed(&context, 0).unwrap_err();
        let mut indexed_seed_1 = root_seed.generate_indexed_seed(&context, 1).unwrap();
        let mut indexed_seed_3_b = root_seed.generate_indexed_seed(&context, 3).unwrap();
        assert!(
            indexed_seed_3
                .seed_mut()
                .buf
                .compare(&mut indexed_seed_3_b.seed_mut().buf)
                == 0
        );
        assert!(
            indexed_seed_3
                .seed_mut()
                .buf
                .compare(&mut indexed_seed_1.seed_mut().buf)
                != 0
        );
    }

    #[test]
    fn it_should_create_a_indexed_pin_seed() {
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let mut pin = generate_random_seed_buf(SEED_SIZE);

        let context = SeedContext::from("HCDEVICE");
        let mut root_seed = RootSeed::new(seed_buf);
        let mut indexed_seed = root_seed.generate_indexed_seed(&context, 3).unwrap();
        let indexed_pin_seed = indexed_seed
            .generate_indexed_pin_seed(&mut pin, TEST_CONFIG)
            .unwrap();
        assert_eq!(SeedType::IndexedPin, indexed_pin_seed.seed().kind);
    }

    #[test]
    fn it_should_create_app_key_from_root_seed() {
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let mut pin = generate_random_seed_buf(SEED_SIZE);

        let context = SeedContext::from("HCDEVICE");
        let mut rs = RootSeed::new(seed_buf);
        let mut ds = rs.generate_indexed_seed(&context, 3).unwrap();
        let mut dps = ds.generate_indexed_pin_seed(&mut pin, TEST_CONFIG).unwrap();
        let mut keybundle_5 = dps.generate_application_key(5).unwrap();

        assert_eq!(crate::SIGNATURE_SIZE, keybundle_5.sign_keys.private.len());
        assert_eq!(SEED_SIZE, keybundle_5.enc_keys.private.len());
        assert_eq!(SeedType::Application, keybundle_5.seed_type);

        let res = dps.generate_application_key(0);
        assert!(res.is_err());

        let mut keybundle_1 = dps.generate_application_key(1).unwrap();
        let mut keybundle_5_b = dps.generate_application_key(5).unwrap();
        assert!(keybundle_5.is_same(&mut keybundle_5_b));
        assert!(!keybundle_5.is_same(&mut keybundle_1));
    }

    #[test]
    fn it_should_roundtrip_mnemonic() {
        let mut seed_buf = SecBuf::with_insecure(SEED_SIZE);
        {
            let mut seed_buf = seed_buf.write_lock();
            seed_buf[0] = 12;
            seed_buf[1] = 70;
            seed_buf[2] = 88;
        }
        let mut seed = Seed::new(seed_buf, SeedType::Root);
        let mnemonic = seed.get_mnemonic().unwrap();
        println!("mnemonic: {:?}", mnemonic);

        let mut seed_2 = Seed::new_with_mnemonic(mnemonic, SeedType::Root).unwrap();
        assert_eq!(seed.kind, seed_2.kind);
        assert_eq!(0, seed.buf.compare(&mut seed_2.buf));
    }

    #[test]
    fn it_should_change_into_typed() {
        // Root
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let seed = Seed::new(seed_buf, SeedType::Root);
        let unknown_seed = seed.into_typed().unwrap();
        let _ = match unknown_seed {
            TypedSeed::Root(typed_seed) => typed_seed,
            _ => unreachable!(),
        };
        // Indexed
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let seed = Seed::new(seed_buf, SeedType::Indexed);
        let unknown_seed = seed.into_typed().unwrap();
        let _ = match unknown_seed {
            TypedSeed::Indexed(typed_seed) => typed_seed,
            _ => unreachable!(),
        };
        // IndexedPin
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let seed = Seed::new(seed_buf, SeedType::IndexedPin);
        let unknown_seed = seed.into_typed().unwrap();
        let _ = match unknown_seed {
            TypedSeed::IndexedPin(typed_seed) => typed_seed,
            _ => unreachable!(),
        };
        // App
        let seed_buf = generate_random_seed_buf(SEED_SIZE);
        let seed = Seed::new(seed_buf, SeedType::Application);
        let maybe_seed = seed.into_typed();
        assert!(maybe_seed.is_err());
    }
}
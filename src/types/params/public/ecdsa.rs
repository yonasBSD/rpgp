use std::io;

use byteorder::WriteBytesExt;
use elliptic_curve::sec1::ToEncodedPoint;

use crate::crypto::ecc_curve::ECCCurve;
use crate::errors::Result;
use crate::ser::Serialize;
use crate::types::{Mpi, MpiRef};

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum EcdsaPublicParams {
    P256 {
        #[cfg_attr(test, proptest(strategy = "tests::p256_pub_gen()"))]
        key: p256::PublicKey,
    },
    P384 {
        #[cfg_attr(test, proptest(strategy = "tests::p384_pub_gen()"))]
        key: p384::PublicKey,
    },
    P521 {
        #[cfg_attr(test, proptest(strategy = "tests::p521_pub_gen()"))]
        key: p521::PublicKey,
    },
    Secp256k1 {
        #[cfg_attr(test, proptest(strategy = "tests::k256_pub_gen()"))]
        key: k256::PublicKey,
    },
    #[cfg_attr(test, proptest(skip))]
    Unsupported { curve: ECCCurve, p: Mpi },
}

impl EcdsaPublicParams {
    pub fn try_from_mpi(p: MpiRef<'_>, curve: ECCCurve) -> Result<Self> {
        match curve {
            ECCCurve::P256 => {
                ensure!(p.len() <= 65, "invalid public key length");
                let mut key = [0u8; 65];
                key[..p.len()].copy_from_slice(p.as_bytes());

                let public = p256::PublicKey::from_sec1_bytes(&key)?;
                Ok(EcdsaPublicParams::P256 { key: public })
            }
            ECCCurve::P384 => {
                ensure!(p.len() <= 97, "invalid public key length");
                let mut key = [0u8; 97];
                key[..p.len()].copy_from_slice(p.as_bytes());

                let public = p384::PublicKey::from_sec1_bytes(&key)?;
                Ok(EcdsaPublicParams::P384 { key: public })
            }
            ECCCurve::P521 => {
                ensure!(p.len() <= 133, "invalid public key length");
                let mut key = [0u8; 133];
                key[..p.len()].copy_from_slice(p.as_bytes());

                let public = p521::PublicKey::from_sec1_bytes(&key)?;
                Ok(EcdsaPublicParams::P521 { key: public })
            }
            ECCCurve::Secp256k1 => {
                ensure!(p.len() <= 65, "invalid public key length");
                let mut key = [0u8; 65];
                key[..p.len()].copy_from_slice(p.as_bytes());

                let public = k256::PublicKey::from_sec1_bytes(&key)?;
                Ok(EcdsaPublicParams::Secp256k1 { key: public })
            }
            _ => Ok(EcdsaPublicParams::Unsupported {
                curve,
                p: p.to_owned(),
            }),
        }
    }

    pub const fn secret_key_length(&self) -> Option<usize> {
        match self {
            EcdsaPublicParams::P256 { .. } => Some(32),
            EcdsaPublicParams::P384 { .. } => Some(48),
            EcdsaPublicParams::P521 { .. } => Some(66),
            EcdsaPublicParams::Secp256k1 { .. } => Some(32),
            EcdsaPublicParams::Unsupported { .. } => None,
        }
    }
}

impl Serialize for EcdsaPublicParams {
    fn to_writer<W: io::Write>(&self, writer: &mut W) -> Result<()> {
        let oid = match self {
            EcdsaPublicParams::P256 { .. } => ECCCurve::P256.oid(),
            EcdsaPublicParams::P384 { .. } => ECCCurve::P384.oid(),
            EcdsaPublicParams::P521 { .. } => ECCCurve::P521.oid(),
            EcdsaPublicParams::Secp256k1 { .. } => ECCCurve::Secp256k1.oid(),
            EcdsaPublicParams::Unsupported { curve, .. } => curve.oid(),
        };

        writer.write_u8(oid.len().try_into()?)?;
        writer.write_all(&oid)?;

        match self {
            EcdsaPublicParams::P256 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                p.as_ref().to_writer(writer)?;
            }
            EcdsaPublicParams::P384 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                p.as_ref().to_writer(writer)?;
            }
            EcdsaPublicParams::P521 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                p.as_ref().to_writer(writer)?;
            }
            EcdsaPublicParams::Secp256k1 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                p.as_ref().to_writer(writer)?;
            }
            EcdsaPublicParams::Unsupported { p, .. } => {
                p.as_ref().to_writer(writer)?;
            }
        }

        Ok(())
    }

    fn write_len(&self) -> usize {
        let oid = match self {
            EcdsaPublicParams::P256 { .. } => ECCCurve::P256.oid(),
            EcdsaPublicParams::P384 { .. } => ECCCurve::P384.oid(),
            EcdsaPublicParams::P521 { .. } => ECCCurve::P521.oid(),
            EcdsaPublicParams::Secp256k1 { .. } => ECCCurve::Secp256k1.oid(),
            EcdsaPublicParams::Unsupported { curve, .. } => curve.oid(),
        };

        let mut sum = 1;
        sum += oid.len();

        match self {
            EcdsaPublicParams::P256 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                sum += p.as_ref().write_len();
            }
            EcdsaPublicParams::P384 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                sum += p.as_ref().write_len();
            }
            EcdsaPublicParams::P521 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                sum += p.as_ref().write_len();
            }
            EcdsaPublicParams::Secp256k1 { key, .. } => {
                let p = Mpi::from_slice(key.to_encoded_point(false).as_bytes());
                sum += p.as_ref().write_len();
            }
            EcdsaPublicParams::Unsupported { p, .. } => {
                sum += p.as_ref().write_len();
            }
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    proptest::prop_compose! {
        pub fn p256_pub_gen()(seed: u64) -> p256::PublicKey {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            p256::SecretKey::random(&mut rng).public_key()
        }
    }

    proptest::prop_compose! {
        pub fn p384_pub_gen()(seed: u64) -> p384::PublicKey {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            p384::SecretKey::random(&mut rng).public_key()
        }
    }

    proptest::prop_compose! {
        pub fn p521_pub_gen()(seed: u64) -> p521::PublicKey {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            p521::SecretKey::random(&mut rng).public_key()
        }
    }

    proptest::prop_compose! {
        pub fn k256_pub_gen()(seed: u64) -> k256::PublicKey {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            k256::SecretKey::random(&mut rng).public_key()
        }
    }
}

use bytes::{Buf, Bytes};

use crate::crypto::public_key::PublicKeyAlgorithm;
use crate::crypto::sym::SymmetricKeyAlgorithm;
use crate::errors::Result;
use crate::parsing::BufParsing;

use super::MpiBytes;

#[cfg(test)]
use proptest::prelude::*;

/// Values comprising a Public Key Encrypted Session Key
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PkeskBytes {
    Rsa {
        mpi: MpiBytes,
    },
    Elgamal {
        first: MpiBytes,
        second: MpiBytes,
    },
    Ecdh {
        public_point: MpiBytes,
        encrypted_session_key: Bytes,
    },
    X25519 {
        /// Ephemeral X25519 public key (32 bytes).
        ephemeral: [u8; 32],
        /// Encrypted and wrapped session key.
        session_key: Bytes,
        /// Set for v3 PKESK only (the sym_alg is not encrypted with the session key for X25519)
        sym_alg: Option<SymmetricKeyAlgorithm>,
    },
    X448 {
        /// Ephemeral X448 public key (56 bytes).
        ephemeral: [u8; 56],
        /// Encrypted and wrapped session key.
        session_key: Bytes,
        /// Set for v3 PKESK only (the sym_alg is not encrypted with the session key for X448)
        sym_alg: Option<SymmetricKeyAlgorithm>,
    },
    Other,
}

impl PkeskBytes {
    pub fn from_buf<B: Buf>(alg: &PublicKeyAlgorithm, version: u8, mut i: B) -> Result<Self> {
        match alg {
            PublicKeyAlgorithm::RSA
            | PublicKeyAlgorithm::RSASign
            | PublicKeyAlgorithm::RSAEncrypt => {
                let mpi = MpiBytes::from_buf(&mut i)?;
                Ok(PkeskBytes::Rsa { mpi })
            }
            PublicKeyAlgorithm::Elgamal | PublicKeyAlgorithm::ElgamalSign => {
                let first = MpiBytes::from_buf(&mut i)?;
                let second = MpiBytes::from_buf(&mut i)?;
                Ok(PkeskBytes::Elgamal { first, second })
            }
            PublicKeyAlgorithm::ECDSA
            | PublicKeyAlgorithm::DSA
            | PublicKeyAlgorithm::DiffieHellman => Ok(PkeskBytes::Other),
            PublicKeyAlgorithm::ECDH => {
                let public_point = MpiBytes::from_buf(&mut i)?;
                let session_key_len = i.read_u8()?;
                let session_key = i.read_take(session_key_len.into())?;

                Ok(PkeskBytes::Ecdh {
                    public_point,
                    encrypted_session_key: session_key,
                })
            }
            PublicKeyAlgorithm::X25519 => {
                // 32 octets representing an ephemeral X25519 public key.
                let ephemeral_public = i.read_array::<32>()?;

                // A one-octet size of the following fields.
                let len = i.read_u8()?;
                if len == 0 {
                    return Err(crate::errors::Error::InvalidInput);
                }

                // The one-octet algorithm identifier, if it was passed (in the case of a v3 PKESK packet).
                let sym_alg = if version == 3 {
                    let alg = i.read_u8().map(SymmetricKeyAlgorithm::from)?;
                    Some(alg)
                } else {
                    None
                };

                let skey_len = if version == 3 { len - 1 } else { len };

                // The encrypted session key.
                let esk = i.read_take(skey_len.into())?;

                Ok(PkeskBytes::X25519 {
                    ephemeral: ephemeral_public,
                    sym_alg,
                    session_key: esk,
                })
            }
            #[cfg(feature = "unstable-curve448")]
            PublicKeyAlgorithm::X448 => {
                // 56 octets representing an ephemeral X448 public key.
                let ephemeral_public = i.read_array::<56>()?;

                // A one-octet size of the following fields.
                let len = i.read_u8()?;
                if len == 0 {
                    return Err(crate::errors::Error::InvalidInput);
                }

                // The one-octet algorithm identifier, if it was passed (in the case of a v3 PKESK packet).
                let sym_alg = if version == 3 {
                    let alg = i.read_u8().map(SymmetricKeyAlgorithm::from)?;
                    Some(alg)
                } else {
                    None
                };

                let skey_len = if version == 3 { len - 1 } else { len };

                // The encrypted session key.
                let esk = i.read_take(skey_len.into())?;

                Ok(PkeskBytes::X448 {
                    ephemeral: ephemeral_public,
                    sym_alg,
                    session_key: esk,
                })
            }
            #[cfg(not(feature = "unstable-curve448"))]
            PublicKeyAlgorithm::X448 => Ok(PkeskBytes::Other),
            PublicKeyAlgorithm::Unknown(_) => Ok(PkeskBytes::Other), // we don't know the format of this data
            _ => unsupported_err!("unsupported algorithm for ESK"),
        }
    }
}

#[cfg(test)]
mod tests {
    use prop::collection;

    use super::*;

    impl Arbitrary for PkeskBytes {
        type Parameters = (PublicKeyAlgorithm, bool);
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary() -> Self::Strategy {
            any::<(PublicKeyAlgorithm, bool)>()
                .prop_flat_map(Self::arbitrary_with)
                .boxed()
        }

        fn arbitrary_with((alg, is_v6): Self::Parameters) -> Self::Strategy {
            match alg {
                PublicKeyAlgorithm::RSA
                | PublicKeyAlgorithm::RSAEncrypt
                | PublicKeyAlgorithm::RSASign => {
                    any::<MpiBytes>().prop_map(|mpi| Self::Rsa { mpi }).boxed()
                }
                PublicKeyAlgorithm::Elgamal | PublicKeyAlgorithm::ElgamalSign => {
                    any::<(MpiBytes, MpiBytes)>()
                        .prop_map(|(first, second)| Self::Elgamal { first, second })
                        .boxed()
                }
                PublicKeyAlgorithm::ECDH => any::<MpiBytes>()
                    .prop_flat_map(|a| (Just(a), collection::vec(0u8..255u8, 1..100)))
                    .prop_map(|(a, b)| Self::Ecdh {
                        public_point: a,
                        encrypted_session_key: b.into(),
                    })
                    .boxed(),
                PublicKeyAlgorithm::X25519 => any::<([u8; 32], SymmetricKeyAlgorithm)>()
                    .prop_flat_map(|(a, b)| (Just(a), Just(b), collection::vec(0u8..255u8, 1..100)))
                    .prop_map(move |(a, b, c)| Self::X25519 {
                        ephemeral: a,
                        session_key: c.into(),
                        sym_alg: (!is_v6).then_some(b),
                    })
                    .boxed(),
                #[cfg(feature = "unstable-curve448")]
                PublicKeyAlgorithm::X448 => any::<([u8; 56], SymmetricKeyAlgorithm)>()
                    .prop_flat_map(|(a, b)| (Just(a), Just(b), collection::vec(0u8..255u8, 1..100)))
                    .prop_map(move |(a, b, c)| Self::X448 {
                        ephemeral: a,
                        session_key: c.into(),
                        sym_alg: (!is_v6).then_some(b),
                    })
                    .boxed(),
                _ => unreachable!("unsupported {:?}", alg),
            }
        }
    }
}

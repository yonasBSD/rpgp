#![allow(clippy::result_large_err)]
use std::{fmt::Debug, fs::File};

use chrono::{DateTime, Utc};
use p256::pkcs8::DecodePrivateKey;
use pgp::{
    adapter::{EcdsaSigner, RsaSigner},
    composed::{Esk, Message, SignedPublicKey, SignedSecretKey},
    crypto::{
        checksum, ecc_curve::ECCCurve, hash::HashAlgorithm, public_key::PublicKeyAlgorithm,
        sym::SymmetricKeyAlgorithm,
    },
    packet::{self, PubKeyInner, PublicKey, SignatureConfig},
    types::{
        EcdhPublicParams, Fingerprint, KeyDetails, KeyId, KeyVersion, Mpi, Password, PkeskBytes,
        PublicKeyTrait, PublicParams, SecretKeyTrait, SignatureBytes,
    },
};

#[derive(Debug, Clone)]
pub struct FakeHsm {
    public_key: PublicKey,

    // data to decrypt() -> data from card
    decrypt_data: Option<(&'static [&'static [u8]], &'static [u8])>,

    // data to card -> data from card
    sign_data: Option<(&'static [u8], &'static [u8])>,
}

impl FakeHsm {
    pub fn with_public_key(public_key: PublicKey) -> Result<Self, pgp::errors::Error> {
        Ok(Self {
            public_key,
            decrypt_data: None,
            sign_data: None,
        })
    }

    pub fn set_fake_decryption_data(
        &mut self,
        input: &'static [&'static [u8]],
        out: &'static [u8],
    ) {
        self.decrypt_data = Some((input, out));
    }

    pub fn set_fake_signing_data(&mut self, input: &'static [u8], out: &'static [u8]) {
        self.sign_data = Some((input, out));
    }

    /// The OpenPGP public key material that corresponds to the key in this CardSlot
    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }
}

impl PublicKeyTrait for FakeHsm {
    fn verify_signature(
        &self,
        hash: HashAlgorithm,
        data: &[u8],
        sig: &SignatureBytes,
    ) -> pgp::errors::Result<()> {
        self.public_key.verify_signature(hash, data, sig)
    }

    fn public_params(&self) -> &PublicParams {
        self.public_key.public_params()
    }

    fn created_at(&self) -> &chrono::DateTime<chrono::Utc> {
        self.public_key.created_at()
    }

    fn expiration(&self) -> Option<u16> {
        self.public_key.expiration()
    }
}

impl KeyDetails for FakeHsm {
    fn version(&self) -> pgp::types::KeyVersion {
        self.public_key.version()
    }

    fn fingerprint(&self) -> Fingerprint {
        self.public_key.fingerprint()
    }

    fn key_id(&self) -> KeyId {
        self.public_key.key_id()
    }

    fn algorithm(&self) -> PublicKeyAlgorithm {
        self.public_key.algorithm()
    }
}

impl SecretKeyTrait for FakeHsm {
    fn create_signature(
        &self,
        _key_pw: &Password,
        _hash: HashAlgorithm,
        data: &[u8],
    ) -> pgp::errors::Result<SignatureBytes> {
        assert_eq!(data, self.sign_data.unwrap().0);

        // XXX: imagine a smartcard producing a signature for `data`, here

        let sig = self.sign_data.unwrap().1; // fake smartcard output

        let mpis = match self.public_key.algorithm() {
            PublicKeyAlgorithm::RSA => vec![Mpi::from_slice(sig)],

            PublicKeyAlgorithm::ECDSA => {
                let mid = sig.len() / 2;

                vec![Mpi::from_slice(&sig[..mid]), Mpi::from_slice(&sig[mid..])]
            }
            PublicKeyAlgorithm::EdDSALegacy => {
                assert_eq!(sig.len(), 64); // FIXME: check curve; add error handling

                vec![Mpi::from_slice(&sig[..32]), Mpi::from_slice(&sig[32..])]
            }

            _ => unimplemented!(),
        };

        Ok(mpis.into())
    }

    fn hash_alg(&self) -> HashAlgorithm {
        self.public_key.public_params().hash_alg()
    }
}

impl FakeHsm {
    pub fn decrypt(
        &self,
        values: &PkeskBytes,
    ) -> pgp::errors::Result<(Vec<u8>, SymmetricKeyAlgorithm)> {
        let decrypted_key = match (self.public_key.public_params(), values) {
            (PublicParams::RSA { .. }, PkeskBytes::Rsa { mpi }) => {
                // The test data in self.decrypt_data must match the parameters
                // (this fake hsm just stores the answer for one request, and it's only legal to
                // call it with the exact set of parameters we have stored)
                assert_eq!(vec![mpi.as_ref()], self.decrypt_data.unwrap().0);

                let _ciphertext = mpi.as_ref();

                // XXX: imagine a smartcard decrypting `_ciphertext`, here

                let dec = self.decrypt_data.unwrap().1; // fake smartcard output

                dec.to_vec()
            }

            (
                PublicParams::ECDH(params),
                PkeskBytes::Ecdh {
                    public_point,
                    encrypted_session_key,
                },
            ) => {
                // The test data in self.decrypt_data must match the parameters
                // (this fake hsm just stores the answer for one request, and it's only legal to
                // call it with the exact set of parameters we have stored)
                assert_eq!(
                    vec![
                        public_point.as_ref(),
                        &[encrypted_session_key.len() as u8],
                        encrypted_session_key
                    ],
                    self.decrypt_data.unwrap().0
                );

                let ciphertext = public_point.as_ref();

                let _ciphertext = if params.curve() == ECCCurve::Curve25519 {
                    assert_eq!(
                        ciphertext[0], 0x40,
                        "Unexpected shape of Cv25519 encrypted data"
                    );

                    // Strip trailing 0x40
                    &ciphertext[1..]
                } else {
                    // For NIST and brainpool: we decrypt the ciphertext as is
                    ciphertext
                };

                // XXX: imagine a smartcard decrypting `_ciphertext`, here

                let dec = self.decrypt_data.unwrap().1; // fake smartcard output

                let shared_secret: [u8; 32] = dec.try_into().expect("must be [u8; 32]");

                let (hash, alg_sym) = match params {
                    EcdhPublicParams::Curve25519 { hash, alg_sym, .. }
                    | EcdhPublicParams::P256 { hash, alg_sym, .. }
                    | EcdhPublicParams::P384 { hash, alg_sym, .. }
                    | EcdhPublicParams::P521 { hash, alg_sym, .. } => (hash, alg_sym),
                    EcdhPublicParams::Brainpool256 { .. }
                    | EcdhPublicParams::Brainpool384 { .. }
                    | EcdhPublicParams::Brainpool512 { .. } => {
                        panic!("unsupported params: {params:?}");
                    }
                    EcdhPublicParams::Unsupported { .. } => {
                        panic!("unsupported params: {params:?}");
                    }
                };
                let decrypted_key: Vec<u8> = pgp::crypto::ecdh::derive_session_key(
                    &shared_secret,
                    encrypted_session_key,
                    encrypted_session_key.len(),
                    params.curve(),
                    *hash,
                    *alg_sym,
                    self.public_key.fingerprint().as_bytes(),
                )?;

                decrypted_key
            }

            _ => unimplemented!(),
        };

        // strip off the leading session key algorithm octet, and the two trailing checksum octets
        let dec_len = decrypted_key.len();
        let (sessionkey, checksum) = (
            &decrypted_key[1..dec_len - 2],
            &decrypted_key[dec_len - 2..],
        );

        // ... check the checksum, while we have it at hand
        checksum::simple(checksum.try_into().unwrap(), sessionkey)?;

        let session_key_algorithm = decrypted_key[0].into();
        Ok((sessionkey.to_vec(), session_key_algorithm))
    }
}

// RSA decryption input to the card
// (the card contains the decryption subkey of `tests/unit-tests/hsm/alice-rsa4096.priv`)
const MPIS_RSA_DECRYPT: &[&[u8]] = &[&[
    0x60, 0x37, 0x72, 0x21, 0xe7, 0xe2, 0x27, 0x97, 0x4d, 0x6e, 0x66, 0x0f, 0xca, 0x09, 0x5a, 0x19,
    0xa9, 0xd2, 0x67, 0xcb, 0x66, 0x15, 0x00, 0xfd, 0xef, 0xe0, 0x0a, 0xa9, 0x4b, 0x38, 0x01, 0x3d,
    0x81, 0xca, 0xfc, 0xe2, 0x4d, 0xd1, 0x4e, 0xd0, 0x37, 0xef, 0xff, 0xad, 0x8c, 0xab, 0xb9, 0xa4,
    0x27, 0x33, 0x23, 0x0b, 0x49, 0xfd, 0x7a, 0xc6, 0x85, 0x24, 0xbe, 0x31, 0x33, 0xe6, 0x06, 0x04,
    0xc1, 0xbb, 0xbb, 0x24, 0x6a, 0x05, 0x75, 0xbd, 0x03, 0xee, 0xc4, 0x45, 0x12, 0xd9, 0xbd, 0xcc,
    0x46, 0x36, 0x43, 0x41, 0x9b, 0x6b, 0xc6, 0x98, 0x0b, 0x0b, 0x3c, 0x40, 0xa4, 0x4c, 0xf3, 0xc8,
    0x7b, 0x05, 0xac, 0x5e, 0x0c, 0x92, 0x4c, 0x9a, 0xb8, 0xd3, 0xc3, 0x81, 0x2a, 0x60, 0x3b, 0xed,
    0xfd, 0x03, 0xdc, 0x2b, 0xc6, 0xfe, 0xe0, 0xe4, 0x97, 0x9c, 0x92, 0xc7, 0x0c, 0x81, 0x1c, 0x86,
    0xa9, 0xe2, 0x19, 0x37, 0xf4, 0x6a, 0x6b, 0xa6, 0x5c, 0xab, 0x31, 0xc4, 0x54, 0x22, 0x96, 0x73,
    0xb4, 0xaf, 0x09, 0x05, 0xae, 0xc7, 0x18, 0xf7, 0xdd, 0x69, 0x8d, 0xe8, 0x1f, 0xc3, 0xfb, 0xd9,
    0x81, 0x9d, 0x7f, 0x03, 0x28, 0xf5, 0xe9, 0x9b, 0x49, 0xc4, 0x75, 0x9b, 0xa4, 0x34, 0x6f, 0x86,
    0x2c, 0x3a, 0x5c, 0xb7, 0xe9, 0x31, 0x89, 0x81, 0xdb, 0x59, 0x98, 0xe0, 0x94, 0x5f, 0x59, 0x24,
    0x80, 0x34, 0xd6, 0x88, 0x8f, 0x3c, 0x7c, 0x22, 0x14, 0xf7, 0x0c, 0xa5, 0xba, 0xf5, 0x79, 0x66,
    0x46, 0x26, 0x94, 0x1f, 0xd5, 0xc7, 0x46, 0xd5, 0x68, 0xbe, 0x07, 0xf1, 0x6b, 0x11, 0xab, 0xc0,
    0xeb, 0xd3, 0x67, 0x3e, 0x01, 0xc7, 0x5b, 0x37, 0xcd, 0x6a, 0xb6, 0xfe, 0x7a, 0x20, 0xfc, 0xe2,
    0xe8, 0x5e, 0xcb, 0x65, 0x26, 0x48, 0x46, 0x5f, 0x55, 0xba, 0x31, 0xf5, 0x2d, 0xb6, 0xd2, 0xf4,
    0xa5, 0xd5, 0xbf, 0xd4, 0x58, 0xf6, 0xc9, 0x81, 0x75, 0x80, 0x10, 0xb8, 0xd2, 0x30, 0xf9, 0xc8,
    0x1c, 0x6a, 0x4d, 0xa5, 0x2e, 0x73, 0x7d, 0xde, 0x27, 0xc5, 0x8f, 0xd5, 0x94, 0x3e, 0x78, 0x12,
    0x7c, 0xe2, 0x8a, 0xa1, 0x0a, 0xd6, 0xdb, 0x1f, 0x0a, 0xbc, 0xdc, 0x8b, 0x63, 0xfe, 0x2a, 0x84,
    0x37, 0x3c, 0x0a, 0x4a, 0x9e, 0xce, 0xab, 0x31, 0x20, 0x9e, 0x73, 0x8a, 0x70, 0x78, 0xe7, 0xe6,
    0x26, 0xac, 0xd0, 0xf9, 0x44, 0xd9, 0x3f, 0x19, 0x40, 0x86, 0xae, 0xae, 0x0c, 0x8a, 0x35, 0xf0,
    0x8e, 0x92, 0x35, 0xd9, 0x99, 0x70, 0x69, 0x9b, 0x41, 0x17, 0x9c, 0x49, 0xb6, 0xd2, 0xda, 0xa4,
    0x95, 0xd6, 0xac, 0x50, 0x09, 0x04, 0xf1, 0x20, 0x5d, 0x29, 0x1b, 0x91, 0xf0, 0x3e, 0x8c, 0x3d,
    0x0b, 0xd2, 0xcd, 0xe8, 0xc3, 0xda, 0x39, 0x0c, 0x7e, 0x11, 0x7e, 0x02, 0x7d, 0x10, 0xfe, 0x25,
    0xe2, 0xfc, 0x9e, 0x8a, 0x63, 0x2f, 0x3f, 0x1b, 0xb7, 0x92, 0x9d, 0x98, 0x94, 0x67, 0xdb, 0x35,
    0x3b, 0xfb, 0x3a, 0x75, 0x1d, 0xe5, 0xa6, 0xd9, 0x9f, 0x7f, 0xcc, 0x20, 0x10, 0x35, 0x50, 0x0b,
    0x9e, 0x77, 0x56, 0xe9, 0x9c, 0x44, 0xb8, 0x61, 0x17, 0xa9, 0x21, 0x24, 0xb8, 0x9c, 0xf1, 0xf2,
    0xbb, 0xf2, 0xe0, 0x8a, 0x26, 0x6d, 0xc2, 0x4b, 0xb8, 0x11, 0xad, 0xad, 0xaf, 0x8c, 0xb4, 0x55,
    0x2d, 0x68, 0xca, 0xb3, 0xdd, 0x3f, 0xfc, 0xdb, 0x4d, 0xe3, 0xfb, 0x2a, 0x74, 0x60, 0xdd, 0xe4,
    0xc4, 0xdd, 0x4d, 0x3f, 0xe8, 0xb7, 0x37, 0x3e, 0xd9, 0xe3, 0x52, 0x8b, 0xbb, 0x74, 0x0d, 0xe0,
    0x53, 0xab, 0xdf, 0xa7, 0x63, 0x51, 0x65, 0x04, 0x16, 0x4f, 0xc3, 0x96, 0xf6, 0x5d, 0xd2, 0x29,
    0x99, 0xa2, 0xaf, 0xb9, 0xaf, 0xcc, 0x2a, 0x70, 0x09, 0x07, 0x15, 0x51, 0x59, 0xe6, 0xdc, 0xa3,
]];

/// RSA decryption result from the card
const CARD_RSA_DECRYPTION_RESULT: &[u8] = &[
    9, 135, 233, 39, 94, 28, 235, 241, 139, 120, 210, 180, 7, 213, 200, 169, 175, 213, 183, 101,
    96, 132, 5, 183, 198, 5, 231, 19, 50, 146, 25, 72, 229, 16, 210,
];

// ECC decryption input to the card
// (the card contains the decryption subkey of `tests/unit-tests/hsm/bob-curve25519.priv`)
const MPIS_ECC_DECRYPT: &[&[u8]] = &[
    &[
        0x40, 0xac, 0x0b, 0xaa, 0x2d, 0x32, 0x22, 0x57, 0x90, 0x51, 0x27, 0x28, 0x19, 0x2b, 0x4b,
        0xbc, 0x56, 0x2f, 0x5b, 0x7d, 0xcf, 0xdb, 0xdf, 0x03, 0xe8, 0x8f, 0x96, 0x5c, 0x2d, 0x37,
        0x84, 0xe6, 0x5e,
    ],
    &[0x30],
    &[
        0xb8, 0xef, 0x94, 0x40, 0xb1, 0x67, 0x3d, 0xd5, 0xa7, 0x88, 0x86, 0xfd, 0xd7, 0x17, 0x23,
        0x25, 0x2d, 0x62, 0x73, 0x70, 0xe2, 0xc1, 0x10, 0xe5, 0x2a, 0xe2, 0x34, 0x57, 0xe8, 0x65,
        0xda, 0xe3, 0x19, 0x80, 0xfe, 0xf3, 0xc8, 0x0d, 0x1e, 0xa2, 0x06, 0x47, 0xd2, 0x30, 0xaa,
        0xde, 0xaf, 0x3b,
    ],
];

/// raw decryption result from the card
const CARD_ECC_DECRYPTION_RESULT: &[u8] = &[
    0x45, 0xb5, 0xfc, 0xf2, 0x9d, 0xfe, 0x81, 0x45, 0xfd, 0x7d, 0xc9, 0xbd, 0xe5, 0xb4, 0xf6, 0x9f,
    0x17, 0xa3, 0x01, 0xaa, 0x10, 0x77, 0xad, 0xa6, 0x4f, 0x61, 0xf9, 0xe9, 0x29, 0xc1, 0x1e, 0x3b,
];

#[test]
fn card_decrypt() {
    let cases = [
        (
            // RSA test case
            "tests/unit-tests/hsm/alice-rsa4096.priv",
            "tests/unit-tests/hsm/msg-to-alice4096.enc",
            MPIS_RSA_DECRYPT,
            CARD_RSA_DECRYPTION_RESULT,
        ),
        (
            // ECC test case
            "tests/unit-tests/hsm/bob-curve25519.priv",
            "tests/unit-tests/hsm/msg-to-bob25519.enc",
            MPIS_ECC_DECRYPT,
            CARD_ECC_DECRYPTION_RESULT,
        ),
    ];

    for case in cases {
        let (keyfile, msgfile, input, out) = case;

        let key_file = File::open(keyfile).unwrap();
        let (mut x, _) = pgp::composed::PublicOrSecret::from_reader_many(key_file).unwrap();
        let key: SignedSecretKey = x.next().unwrap().unwrap().try_into().unwrap();

        let pubkey: SignedPublicKey = key.into();
        let enc_subkey = &pubkey.public_subkeys.first().unwrap().key;

        // Transform subkey packet into primary key packet
        // (This is a hack: FakeHsm wants a primary key packet)
        let as_primary = PublicKey::from_inner(
            PubKeyInner::new(
                enc_subkey.version(),
                enc_subkey.algorithm(),
                *enc_subkey.created_at(),
                enc_subkey.expiration(),
                enc_subkey.public_params().clone(),
            )
            .unwrap(),
        )
        .unwrap();

        let mut hsm = FakeHsm::with_public_key(as_primary).unwrap();
        hsm.set_fake_decryption_data(input, out);

        let (message, _headers) = Message::from_armor_file(msgfile).unwrap();

        let Message::Encrypted { esk, mut edata, .. } = message else {
            panic!("not encrypted");
        };

        let values = if let Esk::PublicKeyEncryptedSessionKey(ref k) = esk[0] {
            k.values().expect("known PKESK version")
        } else {
            panic!("whoops")
        };

        let (session_key, session_key_algorithm) = hsm.decrypt(values).unwrap();
        edata
            .decrypt(&pgp::composed::PlainSessionKey::V3_4 {
                key: session_key,
                sym_alg: session_key_algorithm,
            })
            .unwrap();

        let mut message = Message::from_bytes(edata).unwrap();
        let data = message.as_data_vec().unwrap();

        assert_eq!(data, b"foo bar")
    }
}

const SIGN_RSA_IN: &[u8] = &[
    0xb4, 0x8f, 0x7e, 0x1a, 0x7e, 0x38, 0x38, 0xad, 0x80, 0xcb, 0xc6, 0x10, 0xd9, 0x10, 0xb0, 0x64,
    0xf7, 0x08, 0xa6, 0x7c, 0x64, 0x3c, 0x8e, 0x6c, 0x92, 0x40, 0x75, 0xc4, 0x99, 0xe5, 0xeb, 0x06,
];
const SIGN_RSA_OUT: &[u8] = &[
    0x03, 0x8e, 0x16, 0x3a, 0x5e, 0x27, 0x06, 0x63, 0x45, 0xd6, 0xad, 0x0c, 0xcf, 0xe8, 0xd7, 0x91,
    0xc6, 0x20, 0x86, 0x3f, 0x82, 0x63, 0x66, 0x7e, 0x87, 0x7c, 0x5b, 0xd8, 0x9d, 0x9e, 0x50, 0xc0,
    0xd3, 0x5f, 0xc8, 0xe0, 0x74, 0x1a, 0xf5, 0xbe, 0xe5, 0xab, 0x2f, 0xde, 0xcc, 0xdb, 0x82, 0x1f,
    0xa1, 0x4f, 0xf4, 0xee, 0x1a, 0xa3, 0x45, 0xfb, 0x48, 0x4d, 0x18, 0xd0, 0xf1, 0x50, 0xbd, 0xf4,
    0x52, 0xaf, 0x04, 0x0f, 0xd9, 0x2b, 0xf6, 0x88, 0xb8, 0x95, 0xf8, 0x8f, 0xb3, 0xe1, 0xaf, 0x21,
    0x5d, 0xd9, 0x6f, 0x1e, 0x86, 0xc8, 0xc1, 0xf9, 0x86, 0x82, 0xdb, 0xc2, 0xa6, 0xec, 0x1b, 0x0e,
    0x1f, 0x80, 0x65, 0x4b, 0x83, 0x3a, 0x20, 0x05, 0x8b, 0x83, 0xba, 0x17, 0x90, 0x29, 0x92, 0xc7,
    0x28, 0x8d, 0x38, 0x75, 0x98, 0xfc, 0x42, 0x0b, 0x66, 0xa7, 0x0e, 0x86, 0xdc, 0x7e, 0xca, 0x23,
    0x0b, 0x45, 0x57, 0x5d, 0xa4, 0x67, 0x95, 0x40, 0xe5, 0x24, 0x5d, 0x52, 0x39, 0xdd, 0x76, 0x9e,
    0x66, 0xc8, 0xd6, 0x4e, 0x62, 0x35, 0xea, 0xb6, 0xc0, 0xae, 0x22, 0xcb, 0xfa, 0x6a, 0xb9, 0xd9,
    0x1c, 0xfb, 0x64, 0x77, 0x8c, 0x91, 0x8e, 0xa8, 0x12, 0x4c, 0xa3, 0x47, 0xe7, 0xca, 0x8c, 0x22,
    0xb7, 0xfd, 0x1f, 0xe7, 0x3d, 0xd4, 0x04, 0x51, 0xd9, 0x33, 0x0e, 0x73, 0x51, 0x70, 0x89, 0x5e,
    0x8d, 0xf9, 0x00, 0x00, 0x01, 0x1f, 0x1c, 0x61, 0x85, 0x59, 0xe7, 0xa9, 0xca, 0x34, 0xb8, 0xa8,
    0xc9, 0x8f, 0xa2, 0xa1, 0x5b, 0x7f, 0x5a, 0xf1, 0x39, 0x09, 0x46, 0x54, 0x9a, 0xb4, 0xd5, 0xeb,
    0x70, 0x9d, 0xed, 0x24, 0x77, 0x30, 0xf8, 0x9a, 0x8f, 0x7b, 0xab, 0x2a, 0x95, 0x24, 0x1d, 0xdd,
    0x3e, 0x59, 0x65, 0x8c, 0x82, 0xc4, 0x86, 0x97, 0x7f, 0x07, 0xda, 0xc6, 0xb0, 0xfe, 0x03, 0x32,
    0xb8, 0x03, 0x5f, 0x34, 0x9c, 0xb0, 0x63, 0xaa, 0x56, 0x50, 0x1b, 0x2e, 0x23, 0x7b, 0xb9, 0x84,
    0x91, 0x07, 0x0a, 0x42, 0x23, 0x58, 0x07, 0x94, 0xbe, 0xca, 0xa1, 0x56, 0xe5, 0x57, 0x76, 0x9f,
    0xce, 0xd7, 0xcb, 0xb4, 0xff, 0x78, 0x47, 0x19, 0x37, 0x86, 0x60, 0xdc, 0xf4, 0x81, 0xa7, 0x44,
    0x03, 0x54, 0x1f, 0xda, 0xe5, 0xb7, 0x47, 0xe0, 0x8a, 0x37, 0x71, 0x0e, 0xc9, 0xa4, 0x8d, 0xcd,
    0x78, 0x1b, 0x0c, 0x6f, 0xed, 0xda, 0xaa, 0x48, 0xa3, 0x5d, 0x3e, 0x61, 0x9c, 0x38, 0x3a, 0x40,
    0x99, 0x9b, 0x93, 0x94, 0x06, 0x94, 0xff, 0x40, 0x42, 0xeb, 0x7a, 0xdd, 0x12, 0xea, 0x4e, 0x67,
    0x8e, 0xcd, 0xb6, 0xf2, 0xd0, 0x4c, 0x80, 0x35, 0x6a, 0xff, 0x80, 0x2b, 0xc5, 0x5d, 0x25, 0xb3,
    0xb9, 0xcf, 0x88, 0xf3, 0x14, 0x41, 0xbc, 0x21, 0x35, 0x08, 0xab, 0xfa, 0x9b, 0x1b, 0x63, 0x91,
    0x5d, 0x0b, 0x78, 0xb4, 0x8e, 0x51, 0xc1, 0xd2, 0x3e, 0xa6, 0xfb, 0xe8, 0x86, 0x5e, 0x3a, 0x12,
    0x96, 0x03, 0xb9, 0xf4, 0x0c, 0xfe, 0x27, 0x9d, 0x81, 0xbf, 0x71, 0xa1, 0x3c, 0x88, 0xda, 0x36,
    0x1b, 0x18, 0xc2, 0xa6, 0x69, 0x32, 0xc8, 0xe7, 0x95, 0xee, 0xf5, 0x68, 0x9d, 0x76, 0xdd, 0x60,
    0x25, 0x60, 0x9d, 0x69, 0xe1, 0x84, 0x24, 0x8b, 0xae, 0x77, 0x6b, 0xe6, 0xb7, 0xb1, 0xae, 0x72,
    0x90, 0x5f, 0xa0, 0x61, 0x5a, 0x42, 0x09, 0x56, 0x3f, 0xbd, 0xaf, 0xfb, 0x6b, 0x13, 0x5a, 0xc7,
    0x91, 0xc8, 0xbf, 0x69, 0x2b, 0xca, 0xa7, 0x74, 0x76, 0xd0, 0xd5, 0x3f, 0x4c, 0x55, 0xc6, 0x01,
    0xd8, 0xe6, 0x2a, 0x7f, 0x65, 0xd0, 0x13, 0xc2, 0xd0, 0xc0, 0xb0, 0x1b, 0x10, 0x45, 0x6e, 0xf0,
    0x49, 0x04, 0x37, 0x28, 0xf1, 0xf2, 0xe6, 0x8e, 0x77, 0xb4, 0xba, 0x6e, 0xb0, 0x63, 0x75, 0x53,
];

const SIGN_ECC_IN: &[u8] = &[
    0xfa, 0xa2, 0x5a, 0x97, 0x5b, 0xf9, 0x39, 0xfa, 0xe6, 0x62, 0xe1, 0x74, 0x00, 0x58, 0x4b, 0x88,
    0x0e, 0xf1, 0x66, 0x73, 0xbd, 0x50, 0x36, 0xc0, 0xd2, 0xd9, 0xa0, 0xb9, 0x03, 0x1f, 0xf7, 0xa9,
];

const SIGN_ECC_OUT: &[u8] = &[
    0x8e, 0x71, 0x95, 0x06, 0x3b, 0x7a, 0x2f, 0x07, 0xa7, 0xe0, 0xa0, 0x6a, 0xcb, 0x2a, 0xc7, 0xb7,
    0x63, 0xe8, 0xa6, 0x57, 0xb7, 0x29, 0xb1, 0x8f, 0xb1, 0xab, 0x97, 0xd9, 0x9e, 0x02, 0xce, 0x9a,
    0x3a, 0xdb, 0x3e, 0x1e, 0x40, 0x49, 0xb0, 0xb0, 0xbc, 0xed, 0x42, 0xeb, 0xda, 0x2b, 0xb4, 0x7c,
    0x0d, 0x67, 0x01, 0xfd, 0x0e, 0x3f, 0x9d, 0x56, 0xff, 0x09, 0x9f, 0x5f, 0x44, 0x38, 0xba, 0x0d,
];

#[test]
fn card_sign() {
    let cases = [
        (
            // RSA test case
            "tests/unit-tests/hsm/alice-rsa4096.priv",
            1711230710,
            SIGN_RSA_IN,
            SIGN_RSA_OUT,
        ),
        (
            // ECC test case
            "tests/unit-tests/hsm/bob-curve25519.priv",
            1711230918,
            SIGN_ECC_IN,
            SIGN_ECC_OUT,
        ),
    ];

    for case in cases {
        let (keyfile, sig_creation, input, out) = case;

        let key_file = File::open(keyfile).unwrap();
        let (mut x, _) = pgp::composed::PublicOrSecret::from_reader_many(key_file).unwrap();
        let key: SignedSecretKey = x.next().unwrap().unwrap().try_into().unwrap();

        let pubkey: SignedPublicKey = key.into();

        let mut hsm = FakeHsm::with_public_key(pubkey.primary_key.clone()).unwrap();
        hsm.set_fake_signing_data(input, out);

        const DATA: &[u8] = b"Hello World";

        // -- use hsm signer
        let mut config = SignatureConfig::v4(
            packet::SignatureType::Binary,
            hsm.public_key().algorithm(),
            HashAlgorithm::Sha256,
        );

        config.hashed_subpackets = vec![
            packet::Subpacket::regular(packet::SubpacketData::SignatureCreationTime(
                DateTime::<Utc>::from_timestamp(sig_creation, 0).unwrap(),
            ))
            .unwrap(),
            packet::Subpacket::regular(packet::SubpacketData::Issuer(hsm.key_id())).unwrap(),
        ];

        let signature = config.sign(&hsm, &"".into(), DATA).unwrap();

        signature.verify(&pubkey, DATA).expect("ok");
    }
}

#[test]
fn ecdsa_signer() {
    let inner =
        p256::ecdsa::SigningKey::read_pkcs8_pem_file("tests/unit-tests/hsm/p256.pem").unwrap();

    let signer =
        EcdsaSigner::<_, p256::NistP256>::new(inner, KeyVersion::V4, Default::default()).unwrap();
    const DATA: &[u8] = b"Hello World";

    let mut config = SignatureConfig::v4(
        packet::SignatureType::Binary,
        signer.algorithm(),
        HashAlgorithm::Sha256,
    );

    config.hashed_subpackets = vec![
        packet::Subpacket::regular(packet::SubpacketData::SignatureCreationTime(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        ))
        .unwrap(),
        packet::Subpacket::regular(packet::SubpacketData::Issuer(signer.key_id())).unwrap(),
    ];

    let signature = config.sign(&signer, &Password::empty(), DATA).unwrap();

    signature.verify(&signer, DATA).expect("ok");
}

#[test]
fn rsa_signer() {
    let inner = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::read_pkcs8_pem_file(
        "tests/unit-tests/hsm/rsa.pem",
    )
    .unwrap();

    let signer = RsaSigner::new(inner, KeyVersion::V4, Default::default()).unwrap();
    const DATA: &[u8] = b"Hello World";

    let mut config = SignatureConfig::v4(
        packet::SignatureType::Binary,
        signer.algorithm(),
        HashAlgorithm::Sha256,
    );

    config.hashed_subpackets = vec![
        packet::Subpacket::regular(packet::SubpacketData::SignatureCreationTime(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        ))
        .unwrap(),
        packet::Subpacket::regular(packet::SubpacketData::Issuer(signer.key_id())).unwrap(),
    ];

    let signature = config.sign(&signer, &Password::empty(), DATA).unwrap();

    signature.verify(&signer, DATA).expect("ok");
}

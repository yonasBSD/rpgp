use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::armor::BlockType;
use crate::composed::message::Message;
use crate::composed::shared::is_binary;
use crate::errors::Result;
use crate::parsing_reader::BufReadParsing;
use crate::types::{PkeskVersion, SkeskVersion, Tag};
use crate::{Edata, Esk};

use super::reader::{
    CompressedDataReader, LiteralDataReader, SignatureBodyReader, SignatureOnePassReader,
};

/// Parses a single message level
fn next<'a>(
    mut packets: crate::packet::PacketParser<Box<dyn BufRead + 'a>>,
) -> Result<Option<Message<'a>>> {
    loop {
        let Some(packet) = packets.next_owned() else {
            return Ok(None);
        };
        let mut packet = packet?;

        // Handle 1 OpenPGP Message per loop iteration
        let tag = packet.packet_header().tag();

        match tag {
            Tag::SymKeyEncryptedSessionKey | Tag::PublicKeyEncryptedSessionKey => {
                // (a) Encrypted Message:
                //   - ESK Seq
                //   - Encrypted Data -> OpenPGP Message

                let mut esks = Vec::new();
                let esk = Esk::try_from_reader(&mut packet)?;
                esks.push(esk);

                packets = crate::packet::PacketParser::new(packet.into_inner());
                // Read ESKs unit we find the Encrypted Data
                loop {
                    let Some(packet) = packets.next_owned() else {
                        bail!("missing encrypted data packet");
                    };

                    let mut packet = packet?;
                    let tag = packet.packet_header().tag();
                    match tag {
                        Tag::SymKeyEncryptedSessionKey | Tag::PublicKeyEncryptedSessionKey => {
                            let esk = Esk::try_from_reader(&mut packet)?;
                            esks.push(esk);
                            packets = crate::packet::PacketParser::new(packet.into_inner());
                        }
                        Tag::SymEncryptedData | Tag::SymEncryptedProtectedData => {
                            let edata = Edata::try_from_reader(packet)?;
                            let esk = match edata {
                                Edata::SymEncryptedData { .. } => {
                                    esk_filter(esks, PkeskVersion::V3, SkeskVersion::V4)
                                }
                                Edata::SymEncryptedProtectedData { ref reader } => {
                                    match reader.config() {
                                        crate::packet::SymEncryptedProtectedDataConfig::V1 => {
                                            esk_filter(esks, PkeskVersion::V3, SkeskVersion::V4)
                                        }
                                        crate::packet::SymEncryptedProtectedDataConfig::V2 {
                                            ..
                                        } => esk_filter(esks, PkeskVersion::V6, SkeskVersion::V6),
                                    }
                                }
                            };

                            return Ok(Some(Message::Encrypted { esk, edata }));
                        }
                        Tag::Padding => {
                            // drain reader
                            packet.drain()?;
                            packets = crate::packet::PacketParser::new(packet.into_inner());
                        }
                        Tag::Marker => {
                            // drain reader
                            packet.drain()?;
                            packets = crate::packet::PacketParser::new(packet.into_inner());
                        }
                        _ => {
                            bail!("unexpected tag in an encrypted message: {:?}", tag);
                        }
                    }
                }
            }
            Tag::Signature => {
                // (b) Signed Message
                //   (1) Signature Packet, OpenPGP Message
                //      - Signature Packet
                //      - OpenPGP Message
                let signature =
                    crate::packet::Signature::try_from_reader(packet.packet_header(), &mut packet)?;
                packets = crate::packet::PacketParser::new(packet.into_inner());
                let Some(inner_message) = next(packets)? else {
                    bail!("missing next packet");
                };
                let reader = SignatureBodyReader::new(signature, Box::new(inner_message))?;
                let message = Message::Signed { reader };
                return Ok(Some(message));
            }
            Tag::OnePassSignature => {
                //   (2) One-Pass Signed Message.
                //      - OPS
                //      - OpenPgp Message
                //      - Signature Packet
                let one_pass_signature = crate::packet::OnePassSignature::try_from_reader(
                    packet.packet_header(),
                    &mut packet,
                )?;
                packets = crate::packet::PacketParser::new(packet.into_inner());
                let Some(inner_message) = next(packets)? else {
                    bail!("missing next packet");
                };

                let reader =
                    SignatureOnePassReader::new(&one_pass_signature, Box::new(inner_message))?;
                let message = Message::SignedOnePass {
                    one_pass_signature,
                    reader,
                };
                return Ok(Some(message));
            }
            Tag::CompressedData => {
                // (c) Compressed Message
                //   - Compressed Packet
                let reader = CompressedDataReader::new(packet, false)?;
                let message = Message::Compressed { reader };
                return Ok(Some(message));
            }
            Tag::LiteralData => {
                // (d) Literal Message
                //   - Literal Packet
                let reader = LiteralDataReader::new(packet)?;
                let message = Message::Literal { reader };
                return Ok(Some(message));
            }
            Tag::Padding => {
                // drain reader
                packet.drain()?;
                packets = crate::packet::PacketParser::new(packet.into_inner());
            }
            Tag::Marker => {
                // drain reader
                packet.drain()?;
                packets = crate::packet::PacketParser::new(packet.into_inner());
            }
            _ => {
                bail!("unexpected packet type: {:?}", tag);
            }
        }
    }
}

/// Drop PKESK and SKESK with versions that are not aligned with the encryption container
///
/// An implementation processing an Encrypted Message MUST discard any
/// preceding ESK packet with a version that does not align with the
/// version of the payload.
/// See <https://www.rfc-editor.org/rfc/rfc9580.html#section-10.3.2.1-7>
fn esk_filter(esk: Vec<Esk>, pkesk_allowed: PkeskVersion, skesk_allowed: SkeskVersion) -> Vec<Esk> {
    esk.into_iter()
        .filter(|esk| match esk {
            Esk::PublicKeyEncryptedSessionKey(pkesk) => pkesk.version() == pkesk_allowed,
            Esk::SymKeyEncryptedSessionKey(skesk) => skesk.version() == skesk_allowed,
        })
        .collect()
}

//                     Some(Ok(Message::Encrypted { esk, edata }))
//                 }
//                 Err(err) => Some(Err(err)),
//             };
//         }
//         Tag::Signature => {
//             return match packet.try_into() {
//                 Ok(signature) => {
//                     let message = match next(packets.by_ref()) {
//                         Some(Ok(m)) => Some(Box::new(m)),
//                         Some(Err(err)) => return Some(Err(err)),
//                         None => None,
//                     };

//                     Some(Ok(Message::Signed {
//                         message,
//                         one_pass_signature: None,
//                         signature,
//                     }))
//                 }
//                 Err(err) => Some(Err(err)),
//             };
//         }
//         Tag::OnePassSignature => {
//             return match packet.try_into() {
//                 Ok(p) => {
//                     // TODO: check for `is_nested` marker on OnePassSignatures
//                     let one_pass_signature = Some(p);

//                     let message = match next(packets.by_ref()) {
//                         Some(Ok(m)) => Some(Box::new(m)),
//                         Some(Err(err)) => return Some(Err(err)),
//                         None => None,
//                     };

//                     let signature = if let Some(res) = packets
//                         .next_if(|res| res.as_ref().is_ok_and(|p| p.tag() == Tag::Signature))
//                     {
//                         match res {
//                             Ok(packet) => packet.try_into().expect("peeked"),
//                             Err(e) => return Some(Err(e)),
//                         }
//                     } else {
//                         return Some(Err(format_err!(
//                             "missing signature for, one pass signature"
//                         )));
//                     };

//                     Some(Ok(Message::Signed {
//                         message,
//                         one_pass_signature,
//                         signature,
//                     }))
//                 }
//                 Err(err) => Some(Err(err)),
//             };
//         }
//         Tag::Marker => {
//             // Marker Packets are ignored
//             // see https://www.rfc-editor.org/rfc/rfc9580.html#marker-packet
//         }
//         Tag::Padding => {
//             // Padding Packets are ignored
//             //
//             // "Such a packet MUST be ignored when received."
//             // (See https://www.rfc-editor.org/rfc/rfc9580.html#section-5.14-2)
//         }
//         _ => {
//             return Some(Err(format_err!("unexpected packet {:?}", tag)));
//         }
//     }
// }

//     None
// }

impl<'a> Message<'a> {
    /// Parse a composed message.
    /// Ref: <https://www.rfc-editor.org/rfc/rfc9580.html#name-openpgp-messages>
    fn from_packets(packets: crate::packet::PacketParser<Box<dyn BufRead + 'a>>) -> Result<Self> {
        match next(packets)? {
            Some(message) => Ok(message),
            None => {
                bail!("no valid OpenPGP message found");
            }
        }
    }

    /// Parses a message from the given bytes.
    pub fn from_bytes<R: BufRead + 'a>(source: R) -> Result<Self> {
        let parser = crate::packet::PacketParser::new(Box::new(source) as Box<dyn BufRead>);
        Self::from_packets(parser)
    }

    /// From armored file
    pub fn from_armor_file<P: AsRef<Path>>(path: P) -> Result<(Self, crate::armor::Headers)> {
        let file = std::fs::File::open(path)?;
        Self::from_armor(BufReader::new(file))
    }

    /// From armored string
    pub fn from_string(data: &'a str) -> Result<(Self, crate::armor::Headers)> {
        Self::from_armor(data.as_bytes())
    }

    /// From binary file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        Self::from_bytes(BufReader::new(file))
    }

    /// Armored ascii data.
    pub fn from_armor<R: BufRead + 'a>(input: R) -> Result<(Self, crate::armor::Headers)> {
        let mut dearmor = crate::armor::Dearmor::new(input);
        dearmor.read_header()?;
        // Safe to unwrap, as read_header succeeded.
        let typ = dearmor
            .typ
            .ok_or_else(|| format_err!("dearmor failed to retrieve armor type"))?;

        match typ {
            // Standard PGP types
            BlockType::File | BlockType::Message | BlockType::MultiPartMessage(_, _) => {
                let headers = dearmor.headers.clone(); // FIXME: avoid clone

                if !Self::matches_block_type(typ) {
                    bail!("unexpected block type: {}", typ);
                }

                Ok((Self::from_bytes(BufReader::new(dearmor))?, headers))
            }
            BlockType::PublicKey
            | BlockType::PrivateKey
            | BlockType::Signature
            | BlockType::CleartextMessage
            | BlockType::PublicKeyPKCS1(_)
            | BlockType::PublicKeyPKCS8
            | BlockType::PublicKeyOpenssh
            | BlockType::PrivateKeyPKCS1(_)
            | BlockType::PrivateKeyPKCS8
            | BlockType::PrivateKeyOpenssh => {
                unimplemented_err!("key format {:?}", typ);
            }
        }
    }

    /// Parse from a reader which might contain ASCII amored data or binary data.
    pub fn from_reader<R: BufRead + 'a>(
        mut source: R,
    ) -> Result<(Self, Option<crate::armor::Headers>)> {
        if is_binary(&mut source)? {
            let msg = Self::from_bytes(source)?;
            Ok((msg, None))
        } else {
            let (msg, headers) = Self::from_armor(source)?;
            Ok((msg, Some(headers)))
        }
    }

    fn matches_block_type(typ: BlockType) -> bool {
        matches!(
            typ,
            BlockType::Message | BlockType::MultiPartMessage(_, _) | BlockType::File
        )
    }
}

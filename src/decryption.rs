use std::io::{self, Read, Write};

extern crate sequoia_openpgp as openpgp;
extern crate anyhow;

use openpgp::crypto::SessionKey;
use openpgp::types::SymmetricAlgorithm;
use openpgp::parse::{Parse, stream::*};
use openpgp::policy::Policy;
use openpgp::policy::StandardPolicy as P;
use std::fs::File;
use openpgp::Cert;
use log::trace;

pub struct Decryption {
    policy: Box<dyn Policy>,
    recipient: openpgp::Cert,
    valid_signers: Option<openpgp::Cert>,
}

impl Decryption {
    pub fn new(recipient: openpgp::Cert, valid_signers: Option<openpgp::Cert>) -> Decryption {
        Decryption {
            policy: Box::new(P::new()),
            recipient,
            valid_signers,
        }
    }

    pub fn decrypt<'a>(&'a self, ciphertext: &'a mut (dyn Read + Send + Sync))
        -> Decryptor<'a, &Decryption> {
    
        DecryptorBuilder::from_reader(ciphertext)
            .unwrap()
            .with_policy(self.policy.as_ref(), None, self)
            .unwrap()
    }
}

pub fn decrypt_file(source: &mut File, dest: &mut File, key: &Cert, valid_signers: Option<openpgp::Cert>) -> openpgp::Result<()> {
    decrypt(source, dest, &key, valid_signers)?;
  
    Ok(())
  }

fn decrypt(source: &mut (dyn Read + Send + Sync), sink: &mut (dyn Write + Send + Sync),
  recipient: &openpgp::Cert, signing_cert: Option<openpgp::Cert>) -> openpgp::Result<()> {

    let decryption = Decryption::new(recipient.clone(), signing_cert);

    let mut decrypted = decryption.decrypt(source);

    // Decrypt the data.
    io::copy(&mut decrypted, sink).unwrap();

    Ok(())
}
  

impl VerificationHelper for &Decryption {
    fn get_certs(&mut self, ids: &[openpgp::KeyHandle])
                       -> openpgp::Result<Vec<openpgp::Cert>> {
        // Return public keys for signature verification here.
        trace!("get_certs {:?}", ids);
        let mut certs = Vec::new();
        match &self.valid_signers {
            Some(valid_signer) => {
                for id in ids {
                    for subkey in valid_signer.keys() {
                        if id == &subkey.key_handle() {
                            trace!("Found cert");
                            certs.push(valid_signer.clone());
                        }
                    }
                }
            },
            None => {}
        }
        Ok(certs)
    }

    fn check(&mut self, structure: MessageStructure)
             -> openpgp::Result<()> {
        for layer in structure.iter() {
            match layer {
                MessageLayer::Compression { algo } =>
                    trace!("Compressed using {}", algo),
                MessageLayer::Encryption { sym_algo, aead_algo } =>
                    if let Some(aead_algo) = aead_algo {
                        trace!("Encrypted and protected using {}/{}",
                                    sym_algo, aead_algo);
                    } else {
                        trace!("Encrypted using {}", sym_algo);
                    },
                MessageLayer::SignatureGroup { ref results } =>
                    for result in results {
                        match result {
                            Ok(GoodChecksum { ka, .. }) => {
                                trace!("Good signature from {}", ka.cert());
                                return Ok(())
                            },
                            Err(e) => {
                                trace!("Error: {:?}", e);
                                return Err(anyhow::anyhow!("No valid signature"));
                            }
                        }
                    }
            }
        }
        if self.valid_signers.is_none() {
            return Ok(())
        } else {
            return Err(anyhow::anyhow!("No valid signature"));
        }
    }
}

impl DecryptionHelper for &Decryption {
    fn decrypt<D>(&mut self,
                  pkesks: &[openpgp::packet::PKESK],
                  _skesks: &[openpgp::packet::SKESK],
                  sym_algo: Option<SymmetricAlgorithm>,
                  mut decrypt: D)
                  -> openpgp::Result<Option<openpgp::Fingerprint>>
        where D: FnMut(SymmetricAlgorithm, &SessionKey) -> bool
    {
        let key = self.recipient.keys().unencrypted_secret()
            .with_policy(self.policy.as_ref(), None)
            .for_transport_encryption().next().unwrap().key().clone();

        // The secret key is not encrypted.
        let mut pair = key.into_keypair()?;

        pkesks[0].decrypt(&mut pair, sym_algo)
            .map(|(algo, session_key)| decrypt(algo, &session_key));

        // XXX: In production code, return the Fingerprint of the
        // recipient's Cert here
        Ok(Some(self.recipient.fingerprint()))
    }
}

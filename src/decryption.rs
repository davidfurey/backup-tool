use std::io::{self, Read, Write};

extern crate sequoia_openpgp as openpgp;

use openpgp::crypto::SessionKey;
use openpgp::types::SymmetricAlgorithm;
use openpgp::parse::{Parse, stream::*};
use openpgp::policy::Policy;
use openpgp::policy::StandardPolicy as P;
use std::fs::File;
use openpgp::Cert;

pub struct Decryption {
    policy: Box<dyn Policy>,
    recipient: openpgp::Cert,
}

impl Decryption {
    pub fn new(recipient: openpgp::Cert) -> Decryption {
        Decryption {
            policy: Box::new(P::new()),
            recipient,
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

pub fn decrypt_file(source: &mut File, dest: &mut File, key: &Cert) -> openpgp::Result<()> {
    decrypt(source, dest, &key)?;
  
    Ok(())
  }

fn decrypt(source: &mut (dyn Read + Send + Sync), sink: &mut (dyn Write + Send + Sync),
  recipient: &openpgp::Cert) -> openpgp::Result<()> {

    let decryption = Decryption::new(recipient.clone());

    let mut decrypted = decryption.decrypt(source);

    // Decrypt the data.
    io::copy(&mut decrypted, sink).unwrap();

    Ok(())
}
  

impl VerificationHelper for &Decryption {
    fn get_certs(&mut self, _ids: &[openpgp::KeyHandle])
                       -> openpgp::Result<Vec<openpgp::Cert>> {
        // Return public keys for signature verification here.
        Ok(Vec::new())
    }

    fn check(&mut self, _structure: MessageStructure)
             -> openpgp::Result<()> {
        // Implement your signature verification policy here.
        Ok(())
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

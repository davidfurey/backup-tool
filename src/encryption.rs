use std::fs::File;
use std::io::{self, Write, Read};

extern crate sequoia_openpgp as openpgp;

use openpgp::crypto::SessionKey;
use openpgp::types::{SymmetricAlgorithm, CompressionAlgorithm};
use openpgp::serialize::stream::*;
use openpgp::parse::stream::*;
use openpgp::policy::Policy;
use openpgp::policy::StandardPolicy as P;
use openpgp::types::Timestamp;
use openpgp::Cert;

pub fn encrypt_file(source: &mut File, dest: &mut File, key: &Cert) -> openpgp::Result<()> {
  let p = &P::new();

  encrypt(p, source, dest, &key)?;

  Ok(())
}

pub fn encryptor<'a>(p: &'a dyn Policy, sink: &'a mut (dyn Write + Send + Sync),
          recipient: &'a openpgp::Cert, signing_cert: &'a Option<openpgp::Cert>)
    -> openpgp::Result<openpgp::serialize::stream::Message<'a>>
{
    let recipients =
        recipient.keys().with_policy(p, None).supported().alive().revoked(false)
        .for_transport_encryption();

    // Start streaming an OpenPGP message.
    let mut message = Message::new(sink);

    // We want to encrypt a literal data packet.
    message = Encryptor::for_recipients(message, recipients)
        .build()?;

    message = Compressor::new(message)
      .build()?;

    let signing_key = signing_cert.as_ref().and_then(|x|
        x.keys()
            .with_policy(p, None).alive().revoked(false).for_signing().secret()
            .filter(|ka| ka.has_unencrypted_secret())
            .map(|ka| ka.key())
            .next()
    );

    match signing_key {
        Some(v) => {
            message = Signer::new(message, v.clone().into_keypair().expect("Key was unexpectedly encrypted")).build().unwrap();
        }
        None => {
            if signing_key.is_some() {
                panic!("Unable to find appropriate signing keypair despite certificate being provided")
            }
        }
    }

    // Emit a literal data packet.
    let message = LiteralWriter::new(message)
      .filename("foo")?
      .date(Timestamp::from(1585925313))?
      .build()?;

    Ok(message)
}

fn encrypt(p: &dyn Policy, source: &mut (dyn Read), sink: &mut (dyn Write + Send + Sync),
          recipient: &openpgp::Cert)
    -> openpgp::Result<()>
{
    let recipients =
        recipient.keys().with_policy(p, None).supported().alive().revoked(false)
        .for_transport_encryption();

    // Start streaming an OpenPGP message.
    let message = Message::new(sink);

    // We want to encrypt a literal data packet.
    let message = Encryptor::for_recipients(message, recipients)
        .build()?;

    let message = Compressor::new(message)
      .algo(CompressionAlgorithm::Uncompressed) // todo: 13.5 seconds down to under 8 seconds ?? is it worth it
      .build()?;

    // Emit a literal data packet.
    let mut message = LiteralWriter::new(message)
      .filename("foo")?
      .date(Timestamp::from(1585925313))?
      .build()?;

    // Encrypt the data.
    io::copy(source, &mut message).unwrap();

    // Finalize the OpenPGP message to make sure that all data is
    // written.
    message.finalize()?;

    Ok(())
}

struct Helper<'a> {
    secret: &'a openpgp::Cert,
    policy: &'a dyn Policy,
}

impl<'a> VerificationHelper for Helper<'a> {
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

impl<'a> DecryptionHelper for Helper<'a> {
    fn decrypt<D>(&mut self,
                  pkesks: &[openpgp::packet::PKESK],
                  _skesks: &[openpgp::packet::SKESK],
                  sym_algo: Option<SymmetricAlgorithm>,
                  mut decrypt: D)
                  -> openpgp::Result<Option<openpgp::Fingerprint>>
        where D: FnMut(SymmetricAlgorithm, &SessionKey) -> bool
    {
        let key = self.secret.keys().unencrypted_secret()
            .with_policy(self.policy, None)
            .for_transport_encryption().next().unwrap().key().clone();

        // The secret key is not encrypted.
        let mut pair = key.into_keypair()?;

        pkesks[0].decrypt(&mut pair, sym_algo)
            .map(|(algo, session_key)| decrypt(algo, &session_key));

        // XXX: In production code, return the Fingerprint of the
        // recipient's Cert here
        Ok(Some(self.secret.fingerprint()))
    }
}

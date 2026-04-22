use std::fs::File;
use std::io::{self, Write, Read};

extern crate sequoia_openpgp as openpgp;

use openpgp::types::CompressionAlgorithm;
use openpgp::serialize::stream::*;
use openpgp::policy::Policy;
use openpgp::policy::StandardPolicy as P;
use openpgp::types::Timestamp;
use openpgp::Cert;
use log::trace;

pub fn encrypt_file(source: &mut File, dest: &mut File, key: &Cert, signing_cert: Option<openpgp::Cert>) -> openpgp::Result<()> {
  let p = &P::new();

  encrypt(p, source, dest, &key, signing_cert)?;

  Ok(())
}

fn encrypt(p: &dyn Policy, source: &mut dyn Read, sink: &mut (dyn Write + Send + Sync),
          recipient: &openpgp::Cert, signing_cert: Option<openpgp::Cert>)
    -> openpgp::Result<()>
{
    let recipients =
        recipient.keys().with_policy(p, None).supported().alive().revoked(false)
        .for_transport_encryption();

    // Start streaming an OpenPGP message.
    let mut message = Message::new(sink);

    // We want to encrypt a literal data packet.
    message = Encryptor2::for_recipients(message, recipients)
        .build()?;

    message = Compressor::new(message)
      .algo(CompressionAlgorithm::Uncompressed) // todo: 13.5 seconds down to under 8 seconds ?? is it worth it
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
            trace!("Found signing key");
            message = Signer::new(message, v.clone().into_keypair().expect("Key was unexpectedly encrypted")).build().unwrap();
        }
        None => {
            if signing_cert.is_some() {
                panic!("Unable to find appropriate signing keypair despite certificate being provided")
            }
        }
    }

    // Emit a literal data packet.
    message = LiteralWriter::new(message)
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

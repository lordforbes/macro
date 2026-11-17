use std::sync::Mutex;

use super::*;

const LINK: Uuid = Uuid::from_u128(1);
const OTHER_LINK: Uuid = Uuid::from_u128(2);
const KEY_ID: &str = "arn:aws:kms:us-east-1:123456789012:key/test";

/// Stands in for KMS: one fixed data key, "encrypted" by recording the
/// context it was generated under, and refused under any other context, as
/// KMS refuses it.
#[derive(Default)]
struct FakeKms {
    generated_under: Mutex<Option<HashMap<String, String>>>,
    decrypt_calls: Mutex<u32>,
}

impl DataKeyProvider for FakeKms {
    async fn generate_data_key(
        &self,
        encryption_context: HashMap<String, String>,
    ) -> Result<GeneratedDataKey, DataKeyError> {
        *self.generated_under.lock().unwrap() = Some(encryption_context);
        Ok(GeneratedDataKey {
            plaintext: Zeroizing::new(vec![7; AES_256_KEY_LENGTH]),
            encrypted: vec![1, 2, 3],
            key_id: KEY_ID.to_owned(),
        })
    }

    async fn decrypt_data_key(
        &self,
        key_id: &str,
        encrypted_data_key: &[u8],
        encryption_context: HashMap<String, String>,
    ) -> Result<Zeroizing<Vec<u8>>, DataKeyError> {
        *self.decrypt_calls.lock().unwrap() += 1;
        let generated_under = self.generated_under.lock().unwrap().clone();
        if key_id != KEY_ID
            || encrypted_data_key != [1, 2, 3]
            || generated_under != Some(encryption_context)
        {
            return Err(DataKeyError);
        }
        Ok(Zeroizing::new(vec![7; AES_256_KEY_LENGTH]))
    }
}

fn secret() -> MailboxSecret {
    MailboxSecret::parse("correct horse battery staple".to_owned()).unwrap()
}

#[tokio::test]
async fn a_secret_decrypts_for_the_link_it_was_sealed_for() {
    let cipher = MailboxSecretCipher::new(FakeKms::default());

    let envelope = cipher.encrypt(LINK, &secret()).await.unwrap();
    let opened = cipher.decrypt(LINK, &envelope).await.unwrap();

    assert_eq!(opened.expose(), "correct horse battery staple");
    assert_eq!(envelope.kms_key_id, KEY_ID);
    assert_eq!(envelope.encryption_version, ENCRYPTION_VERSION);
    assert!(
        !envelope
            .secret_ciphertext
            .windows(5)
            .any(|window| window == b"horse")
    );
}

#[tokio::test]
async fn the_kms_context_names_the_purpose_and_the_link() {
    let kms = FakeKms::default();
    let cipher = MailboxSecretCipher::new(kms);

    cipher.encrypt(LINK, &secret()).await.unwrap();

    let context = cipher
        .data_keys
        .generated_under
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert_eq!(
        context.get("macro:purpose").map(String::as_str),
        Some("generic-mailbox-secret")
    );
    assert_eq!(context.get("macro:email-link-id"), Some(&LINK.to_string()));
}

#[tokio::test]
async fn a_row_copied_to_another_link_does_not_decrypt() {
    let cipher = MailboxSecretCipher::new(FakeKms::default());
    let envelope = cipher.encrypt(LINK, &secret()).await.unwrap();

    let result = cipher.decrypt(OTHER_LINK, &envelope).await;

    assert!(matches!(result, Err(MailboxSecretCipherError::Kms(_))));
}

#[tokio::test]
async fn a_tampered_ciphertext_does_not_decrypt() {
    let cipher = MailboxSecretCipher::new(FakeKms::default());
    let mut envelope = cipher.encrypt(LINK, &secret()).await.unwrap();
    envelope.secret_ciphertext[0] ^= 1;

    let result = cipher.decrypt(LINK, &envelope).await;

    assert!(matches!(
        result,
        Err(MailboxSecretCipherError::DecryptionFailed)
    ));
}

#[tokio::test]
async fn an_unknown_version_is_refused_without_calling_kms() {
    let cipher = MailboxSecretCipher::new(FakeKms::default());
    let mut envelope = cipher.encrypt(LINK, &secret()).await.unwrap();
    envelope.encryption_version = 2;

    let result = cipher.decrypt(LINK, &envelope).await;

    assert!(matches!(
        result,
        Err(MailboxSecretCipherError::UnsupportedVersion(2))
    ));
    assert_eq!(*cipher.data_keys.decrypt_calls.lock().unwrap(), 0);
}

#[test]
fn a_secret_never_prints() {
    assert_eq!(format!("{:?}", secret()), "MailboxSecret(redacted)");
}

#[test]
fn a_secret_keeps_its_spaces_but_refuses_empty_and_nul() {
    assert_eq!(
        MailboxSecret::parse(" pass word ".to_owned())
            .unwrap()
            .expose(),
        " pass word "
    );
    assert!(MailboxSecret::parse(String::new()).is_err());
    assert!(MailboxSecret::parse("pass\0word".to_owned()).is_err());
}

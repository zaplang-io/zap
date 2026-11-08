# Golden fixture: AEAD symmetric encryption (ticket #772, D-0135).
#
# crypto.aes256gcm_encrypt/decrypt and crypto.chacha20poly1305_encrypt/decrypt
# over OpenSSL EVP, mirroring Python's cryptography AESGCM / ChaCha20Poly1305.
# Key is 32 bytes, nonce 12, and the 16-byte tag is appended (output =
# ciphertext || tag). Verified against the NIST AES-256-GCM zero vector and the
# RFC 8439 section 2.8.2 ChaCha20-Poly1305 vector; round-trips with/without AAD;
# and every authentication failure or bad length raises a catchable ValueError
# (decrypt never returns unauthenticated plaintext). KAT-backed walled garden.

def main():
    auto zkey = crypto.unhex(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    auto znonce = crypto.unhex("000000000000000000000000")

    # NIST AES-256-GCM: zero key/nonce, empty plaintext and AAD -> 16-byte tag.
    assert crypto.hex(crypto.aes256gcm_encrypt(zkey, znonce, b"", b"")) == "530f8afbc74536b9a963b4f1c4cb738b"

    # RFC 8439 section 2.8.2: ChaCha20-Poly1305 AEAD vector.
    auto key = crypto.unhex(
        "808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f",
    )
    auto nonce = crypto.unhex("070000004041424344454647")
    auto aad = crypto.unhex("50515253c0c1c2c3c4c5c6c7")
    auto pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it."
    auto out = crypto.chacha20poly1305_encrypt(key, nonce, pt, aad)
    assert crypto.hex(out) == "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b61161ae10b594f09e26a7e902ecbd0600691"
    # Decrypt the RFC vector back to the exact plaintext.
    assert crypto.chacha20poly1305_decrypt(key, nonce, out, aad) == pt

    # AES-256-GCM round-trip with AAD.
    auto g = crypto.aes256gcm_encrypt(zkey, znonce, b"secret", b"hdr")
    assert crypto.aes256gcm_decrypt(zkey, znonce, g, b"hdr") == b"secret"
    # aad defaults to empty.
    auto g2 = crypto.aes256gcm_encrypt(zkey, znonce, b"nohdr")
    assert crypto.aes256gcm_decrypt(zkey, znonce, g2) == b"nohdr"
    # str plaintext is accepted (shared ZapStr byte ABI); round-trips to bytes.
    assert crypto.aes256gcm_decrypt(zkey, znonce, crypto.aes256gcm_encrypt(zkey, znonce, "hi")) == b"hi"

    # --- authentication failures raise a catchable ValueError ---
    # Wrong AAD.
    auto raised = False
    try:
        crypto.chacha20poly1305_decrypt(key, nonce, out, b"wrong aad")
    except ValueError:
        raised = True
    assert raised

    # Wrong key (ciphertext produced under a different key).
    raised = False
    auto other = crypto.aes256gcm_encrypt(zkey, znonce, b"secret", b"hdr")
    try:
        crypto.aes256gcm_decrypt(key, nonce, other, b"hdr")
    except ValueError:
        raised = True
    assert raised

    # --- parameter validation ---
    # Bad key length.
    raised = False
    try:
        crypto.aes256gcm_encrypt(b"shortkey", znonce, b"x")
    except ValueError:
        raised = True
    assert raised

    # Bad nonce length.
    raised = False
    try:
        crypto.aes256gcm_encrypt(zkey, b"shortnonce", b"x")
    except ValueError:
        raised = True
    assert raised

    # Ciphertext too short to hold a tag.
    raised = False
    try:
        crypto.aes256gcm_decrypt(zkey, znonce, b"tiny")
    except ValueError:
        raised = True
    assert raised

    print("crypto.aead: PASS")

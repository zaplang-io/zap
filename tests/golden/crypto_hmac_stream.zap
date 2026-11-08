# Golden fixture: incremental/streaming HMAC (ticket #766, D-0133).
#
# `crypto.hmac_new(hash_name, key)` returns an opaque handle; `hmac_update(h,
# data)` feeds input; `hmac_hexdigest(h)`/`hmac_digest(h)` finalize (lowercase
# hex / raw bytes) non-destructively, mirroring the streaming-hash handle.
#
# The streamed digest must equal the one-shot `hmac_sha256`/`hmac_sha512` for
# the same key/message (which are themselves pinned against RFC 4231 elsewhere),
# across chunk-boundary splits, and finalize must be non-destructive. One RFC
# 4231 known-answer vector (Test Case 2) is printed directly. Intent docs.

def main():
    auto msg = "The quick brown fox jumps over the lazy dog"

    # --- RFC 4231 Test Case 2 via the streaming path (printed KAT) ---
    # key="Jefe", data="what do ya want for nothing?", HMAC-SHA256.
    auto tc2 = crypto.hmac_new("sha256", "Jefe")
    crypto.hmac_update(tc2, "what do ya want for nothing?")
    print(crypto.hmac_hexdigest(tc2))

    # --- Streaming == one-shot (SHA-256) ---
    auto h = crypto.hmac_new("sha256", "key")
    crypto.hmac_update(h, msg)
    assert crypto.hmac_hexdigest(h) == crypto.hmac_sha256("key", msg)
    # Raw digest equals the one-shot raw digest, and is 32 bytes.
    assert crypto.hmac_digest(h) == crypto.hmac_sha256_raw("key", msg)
    assert len(crypto.hmac_digest(h)) == 32

    # --- Non-destructive finalize: digest twice, then keep updating ---
    assert crypto.hmac_hexdigest(h) == crypto.hmac_sha256("key", msg)

    # --- Chunk-boundary splits equal the one-shot digest (SHA-256) ---
    auto s = crypto.hmac_new("sha256", "key")
    crypto.hmac_update(s, msg[0:1])
    crypto.hmac_update(s, msg[1:10])
    crypto.hmac_update(s, msg[10:43])
    assert crypto.hmac_hexdigest(s) == crypto.hmac_sha256("key", msg)

    # --- Streaming == one-shot (SHA-512), including a split ---
    auto h5 = crypto.hmac_new("sha512", "key")
    crypto.hmac_update(h5, msg[0:20])
    crypto.hmac_update(h5, msg[20:43])
    assert crypto.hmac_hexdigest(h5) == crypto.hmac_sha512("key", msg)
    assert len(crypto.hmac_digest(h5)) == 64

    # --- str and bytes key/data are equivalent ---
    auto hb = crypto.hmac_new("sha256", b"key")
    crypto.hmac_update(hb, b"The quick brown fox jumps over the lazy dog")
    assert crypto.hmac_hexdigest(hb) == crypto.hmac_sha256("key", msg)

    # --- A key longer than the block is hashed down (RFC 2104); streaming and
    #     one-shot must still agree ---
    auto longkey = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    auto hl = crypto.hmac_new("sha256", longkey)
    crypto.hmac_update(hl, msg)
    assert crypto.hmac_hexdigest(hl) == crypto.hmac_sha256(longkey, msg)

    # --- Unknown hash_name raises a catchable ValueError ---
    auto raised = False
    try:
        crypto.hmac_new("sha3", "key")
    except ValueError:
        raised = True
    assert raised

    print("crypto.hmac streaming: PASS")

# Golden fixture: private-key PEM builders from private JWK components
# (ticket #778, D-0136). Symmetric counterparts to the *_public_key_pem builders
# (#643). Each builds a PKCS#8 PEM private key; OpenSSL does all ASN.1 and
# validation. Proven here by: key_info reports a private key of the right
# kind/size, and a signature made with the built private PEM verifies under the
# public PEM built from the same keypair's public components. The fixed vectors
# are disposable keys generated with OpenSSL. Malformed components raise a
# catchable ValueError. Intent documentation in code.

def main():
    auto msg = b"zap #778 private-key PEM round-trip"

    # ---- RSA-2048 (CRT components) ----
    auto n = crypto.unhex(
        "9a82f840a11f467cf0c2b8a4c3a636f467bcb1c6b4584d15a3b3dcba27ffa777b80721e2cd04410b46da1f02707c51828ff84d1afcd177c1f265eeb879b95bb8f1039244f78cfac7544823fdfcffb176c0304e9d7ef93fae4b1a2a21bc400c04b0b652173d9d480de5a9add37a3546d057e2388ddfd3265667aeb5d9b474a71e9977734b35a99a33de89687bd3cd9c0a6667d5f64ae56432ea4c69570760a4a7db1a8e81bd9f31dc974a0044443853793095ac2220f038ad48367411e0681fd316b37769d1ad2b216ec2816bfb4c97288f3cee2975dce3db9e40e830b6315b055829d67f573550188e2f69594520676a837953b69a67a441300192d6e6dc9029",
    )
    auto e = crypto.unhex("010001")
    auto d = crypto.unhex(
        "0d8476be84ba0d3b21ff0f134f108c0f3ec219d575f2a1afca761251f0ece3544c73c749a5c10105c109951bec065c452d5e3e34a5b8979f0e9b4f52b1477f9842f19a8ad11372df1c67014b17a6b0fa601ef9c9ba9188ba3154d529c563df094c026f551c2d55dfed5f3657618226b35474f498a8e3ba9f658bb9da1578fd23c3db2c42f371b5b699c482f41958423cc86ebce8139e4628ac155ae1014e0a496fb6ddeb34301aeb34bbd2fda2dcba9d9da5d6aac59594aafda0b52c5bd262268f8f0d4a407629a3f0d2dee2cb311ea2da59e2115890df6c0f19fcb30bba35d395506d14af1b333541ccb9e5f9ad1a4c65fcb0bafa34b4d7f68f09a9620303cf",
    )
    auto p = crypto.unhex(
        "cea9752630e4e3ff44d7ad28df6aadd61e4a3a530dd1d8857cb7e9b33d1e4519bc84a5b91fa3761358abb429f848d7864ff1d8e4edd490a4500e7676e99f40ee63b132b5bbe0cae5289344541553c4524003879599ad522ed01a95325a22ed04202883a5a0c019205513e51a521c3204d6341602961dc7268a0891aee4fa7cf7",
    )
    auto q = crypto.unhex(
        "bf663e5877e9c952f68f9bff4a3000eed7f44aac640487e82d2be2b95ebd892b7ce51703f3b837daac6eefa27683e10548a76211ad57f79359319f1f8d4f949eecab4c1456ca4020a8e9eec5289da24819a7face0e23a5e78a868246bd83fd8a7d0e46db66ac7a2b69eed02fb2b1e017f374c2d7730311b1ae4cc200a9bfb3df",
    )
    auto dp = crypto.unhex(
        "6bed4fc2fc31ed6838d9b212f06ad412488b575e5c8f44d6a1c5a284fadc3e162cbfd1949693556cd1180ae50847725730950b42b37a1977d65f573aced35ea3a0e8bfa4ed537e62a30815dd1bae3d8efee35901a5da405b364f4baf9a273744ba4eae41e76bab33997ae4611494a902a1a35e0347c69164ed1d8087a809b10d",
    )
    auto dq = crypto.unhex(
        "4fd4ce7649f5ba221f7300c6f76394500ac4fb0c725f2d6c9ac2ae3b5117cf09d37b1e2168db5278a3e9558c36d084df965132675b1cd0c7a5b13bc4864546210f67d740b87e7d46e20d48a8f56ae2501e5b6111e6f5d971fce6639a2bdbccb9a1b57dba6f55cb532649148e80830e8c0ee0899021898b2cc81d7de196ebbd93",
    )
    auto qi = crypto.unhex(
        "c04fb96557f0d0ce140781a0c4c7abc8cae59b73b58bc0127cef968b8d807f0015d234267eb16181a8259b1f0352ad2b15bb7cb0aa6476bf9cc46aac9c90ec067b46aa85f9bd5b1921334437216fe2896911c6e7aef6b1c132b882d912b61f6db6acf01fd4304aab0174c66bed50b0c4b0f8ce62a18785a518a3a3475a46fd5f",
    )

    auto rsa_priv = crypto.rsa_private_key_pem(n, e, d, p, q, dp, dq, qi)
    auto rsa_pub = crypto.rsa_public_key_pem(n, e)
    auto rk = crypto.key_info(rsa_priv)
    assert rk.kind == "RSA"
    assert rk.bits == 2048
    assert rk.is_private
    # Sign with the built private key, verify under the public key.
    assert crypto.rsa_sha256_verify(rsa_pub, msg, crypto.rsa_sha256_sign(rsa_priv, msg))

    # ---- P-256 (scalar d; public point x,y) ----
    auto ec_d = crypto.unhex(
        "809dfa58bcba86e18647149b8b313f9f4658df8c8f4c366b0fb30405b17548b8",
    )
    auto ec_x = crypto.unhex(
        "8ecaf147c58c75977408cf75620d650d7a0349fe816960519157faab5f3c3304",
    )
    auto ec_y = crypto.unhex(
        "d7d25291fea4ecfe92745549e68ae3fe6c8f5cc8ec4f7855206c628e16ea0545",
    )
    auto ec_priv = crypto.p256_private_key_pem(ec_d)
    auto ec_pub = crypto.p256_public_key_pem(ec_x, ec_y)
    auto ek = crypto.key_info(ec_priv)
    assert ek.kind == "EC"
    assert ek.curve == "P-256"
    assert ek.is_private
    assert crypto.p256_sha256_verify(ec_pub, msg, crypto.p256_sha256_sign(ec_priv, msg))

    # ---- Ed25519 (seed d; public x) ----
    auto ed_d = crypto.unhex(
        "d0a8def5c079fbae12ec72abee15497519a94fd97528965dd42f7ee2a87cb0ea",
    )
    auto ed_x = crypto.unhex(
        "0dd2be6e4ce8bc329ccdc7fdba56ff9a6048c37e467b553175225bc3cf84a8b5",
    )
    auto ed_priv = crypto.ed25519_private_key_pem(ed_d)
    auto ed_pub = crypto.ed25519_public_key_pem(ed_x)
    auto dk = crypto.key_info(ed_priv)
    assert dk.kind == "Ed25519"
    assert dk.is_private
    assert crypto.ed25519_verify(ed_pub, msg, crypto.ed25519_sign(ed_priv, msg))

    # ---- malformed components raise a catchable ValueError ----
    # Ed25519 seed wrong length.
    auto raised = False
    try:
        crypto.ed25519_private_key_pem(crypto.unhex("00112233"))
    except ValueError:
        raised = True
    assert raised

    # P-256 scalar wrong length.
    raised = False
    try:
        crypto.p256_private_key_pem(crypto.unhex("00112233"))
    except ValueError:
        raised = True
    assert raised

    # RSA with an inconsistent CRT coefficient.
    raised = False
    try:
        crypto.rsa_private_key_pem(n, e, d, p, q, dp, dq, crypto.unhex("01"))
    except ValueError:
        raised = True
    assert raised

    print("crypto.private_key_pem: PASS")

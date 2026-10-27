# Golden fixture: AWS Signature Version 4 signing-key derivation (ticket #153).
#
# The motivating use of raw-bytes HMAC-SHA256: SigV4 derives its signing key by
# chaining HMAC-SHA256 where each step's RAW digest is the key for the next.
# This is AWS's published worked example ("Deriving the signing key", secret key
# "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", 2015-08-30, us-east-1, iam):
#
#   kDate    = HMAC("AWS4" + secret, "20150830")
#   kRegion  = HMAC(kDate, "us-east-1")
#   kService = HMAC(kRegion, "iam")
#   kSigning = HMAC(kService, "aws4_request")
#   signature = hex(HMAC(kSigning, stringToSign))
#
# AWS publishes kSigning = c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9
#   so crypto.sha256(kSigning) = 30ded1dcf033502869c8051e7ccce37762f623ba1660a832b4f1221f07d07e27
# and the final signature for AWS's published stringToSign is
#   5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7
# Reproducing that end-to-end proves hmac_sha256_raw chains byte-for-byte.
def main():
    auto kDate = crypto.hmac_sha256_raw(
        "AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830",
    )
    auto kRegion = crypto.hmac_sha256_raw(kDate, "us-east-1")
    auto kService = crypto.hmac_sha256_raw(kRegion, "iam")
    auto kSigning = crypto.hmac_sha256_raw(kService, "aws4_request")
    print(crypto.sha256(kSigning))
    auto sts = "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/iam/aws4_request\nf536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
    print(crypto.hmac_sha256(kSigning, sts))

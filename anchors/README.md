# Pinned trust anchors

Roots used to verify the RFC 3161 timestamp tokens stored beside each day
file. They are embedded into the binary at build time, so verification years
from now does not depend on hunting for a certificate.

Pinned 2026-10-22, before any dispute. That date is the point: a root pinned
in advance is checkable against what the authority serves later.

| File | Subject | Expires | SHA-256 fingerprint |
| --- | --- | --- | --- |
| `digicert.pem` | DigiCert Trusted Root G4 | 2038-01-15 | `55:2F:7B:DC:F1:A7:AF:9E:6C:E6:72:01:7F:4F:12:AB:F7:72:40:C7:8E:76:1A:C2:03:D1:D9:D2:0A:C8:99:88` |
| `dfn.pem` | DFN-Verein Community Root CA 2022 | 2042-01-21 | `3C:DC:2C:9E:9E:5A:36:CB:58:88:FD:17:96:CB:91:2F:84:62:53:B6:82:C1:B3:20:57:53:20:33:51:0C:7B:B6` |

## Provenance, and why it differs

**`digicert.pem`** was fetched from DigiCert's official distribution point,
<https://cacerts.digicert.com/DigiCertTrustedRootG4.crt.pem>. It is an
independent trust anchor in the ordinary sense.

**`dfn.pem`** was extracted from a live timestamp token, because the DFN-Verein
Community Root CA 2022 is in no system trust store and we could find it at no
published URL. That makes the DFN anchor **self-referential**: an adversary
able to forge a token could have forged the root alongside it.

This is the reason two authorities are used rather than one. DigiCert's
independently rooted token is what carries the cryptographic weight; DFN's is
the one a German reader will recognise, and it is corroborated by DigiCert
agreeing about the same digest at the same time.

If this ever has to be relied on, ask DFN-CERT for the root and compare the
fingerprint above.

## Checking

```sh
openssl x509 -in anchors/dfn.pem -noout -subject -enddate -fingerprint -sha256
openssl ts -verify -data <day>.jsonl -in <day>.dfn.tsr -CAfile anchors/dfn.pem
```

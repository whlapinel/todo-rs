$version: "2"

namespace common

use aws.protocols#restJson1

@restJson1
service Listeria {
    version: "2026-04-14"
    resources: [
        List
    ]
}

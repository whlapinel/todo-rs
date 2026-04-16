$version: "2.0"

namespace common

string UserId

resource User {
    identifiers: {
        userId: UserId
    }
    resources: [
        List
    ]
}

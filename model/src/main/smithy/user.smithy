$version: "2.0"

namespace common

string UserId

resource User {
    identifiers: {
        userId: UserId
    }
    properties: {
        id: String
        firstName: String
        lastName: String
    }
    resources: [
        List
    ]
}

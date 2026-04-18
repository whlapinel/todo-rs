$version: "2"

namespace common

resource User {
    identifiers: {
        userId: String
    }
    properties: {
        firstName: String
        lastName: String
    }
    read: GetUser
    list: ListUsers
    create: CreateUser
    update: UpdateUser
    resources: [
        List
    ]
}

structure UserSummary for User {
    @required
    $userId

    @required
    $firstName

    @required
    $lastName
}

list Users {
    member: UserSummary
}

@http(method: "POST", uri: "/users")
operation CreateUser {
    input := for User {
        @required
        $firstName

        @required
        $lastName
    }

    output := for User {
        @required
        $userId
    }

    errors: [
        ListeriaError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}")
operation GetUser {
    input := for User {
        @required
        @httpLabel
        $userId
    }

    output := for User {
        @required
        $userId

        @required
        $firstName

        @required
        $lastName
    }

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}")
operation UpdateUser {
    input := for User {
        @required
        @httpLabel
        $userId

        @required
        $firstName

        @required
        $lastName
    }

    output := {}

    errors: [
        ListeriaError
    ]
}

@readonly
@http(method: "GET", uri: "/users")
operation ListUsers {
    input := {}

    output := {
        @required
        users: Users
    }

    errors: [
        ListeriaError
    ]
}

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
    update: UpdateUser
    resources: [
        Item
    ]
    operations: [
        ListItemsDue
        CreateTemplate
        ListTemplates
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
        PeoplesRepublicOfListsError
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
        PeoplesRepublicOfListsError
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
        PeoplesRepublicOfListsError
    ]
}

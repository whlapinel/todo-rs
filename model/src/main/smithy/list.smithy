$version: "2"

namespace common

resource List {
    identifiers: {
        listId: String
        userId: String
    }
    properties: {
        name: String
    }
    resources: [
        Item
    ]
    read: GetList
    list: ListLists
    create: CreateList
    update: UpdateList
}

@http(method: "POST", uri: "/users/{userId}/lists")
operation CreateList {
    input := for List {
        @required
        @httpLabel
        $userId

        @required
        $name
    }

    output := for List {
        @required
        $listId
    }

    errors: [
        ListeriaError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/lists/{listId}")
operation GetList {
    input := for List {
        @required
        @httpLabel
        $listId

        @required
        @httpLabel
        $userId
    }

    output := for List {
        $name
    }

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/lists/{listId}")
operation UpdateList {
    input := for List {
        @required
        @httpLabel
        $userId

        @required
        @httpLabel
        $listId

        @required
        $name
    }

    output := {}

    errors: [
        ListeriaError
    ]
}

structure ListSummary {
    @required
    listId: String

    @required
    userId: String

    @required
    name: String
}

list Lists {
    member: ListSummary
}

@readonly
@http(method: "GET", uri: "/users/{userId}/lists")
operation ListLists {
    input := for List {
        @required
        @httpLabel
        $userId
    }

    output := {
        @required
        lists: Lists
    }

    errors: [
        ListeriaError
    ]
}

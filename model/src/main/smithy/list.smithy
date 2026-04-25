$version: "2"

namespace common

resource List {
    identifiers: {
        listId: String
        userId: String
    }
    properties: {
        name: String
        hasTasks: Boolean
    }
    resources: [
        Item
    ]
    read: GetList
    list: ListLists
    create: CreateList
    update: UpdateList
    delete: DeleteList
}

@http(method: "POST", uri: "/users/{userId}/lists")
operation CreateList {
    input := for List {
        @required
        @httpLabel
        $userId

        @required
        $name

        $hasTasks
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
        $hasTasks
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

        $hasTasks
    }

    output := {}

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/lists/{listId}")
operation DeleteList {
    input := for List {
        @required
        @httpLabel
        $userId

        @required
        @httpLabel
        $listId
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

    hasTasks: Boolean
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

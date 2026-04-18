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

structure TodoList {
    @required
    listId: String

    @required
    userId: String

    @required
    name: String
}

list Lists {
    member: TodoList
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

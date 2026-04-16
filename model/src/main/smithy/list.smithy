$version: "2"

namespace common

string ListName

string ListId

resource List {
    identifiers: {
        listId: ListId
        userId: UserId
    }
    properties: {
        name: ListName
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
    listId: ListId

    @required
    userId: UserId

    @required
    name: ListName
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

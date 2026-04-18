$version: "2"

namespace common

string ItemId

resource Item {
    identifiers: {
        itemId: String
        listId: String
        userId: String
    }
    properties: {
        name: String
        dueDate: Timestamp
    }
    read: GetItem
    list: ListItems
    create: CreateItem
}

@http(method: "POST", uri: "/users/{userId}/lists/{listId}/items")
operation CreateItem {
    input := for Item {
        @required
        @httpLabel
        $userId

        @required
        @httpLabel
        $listId

        @required
        $name

        $dueDate
    }

    output := for Item {
        @required
        $itemId
    }

    errors: [
        ListeriaError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/lists/{listId}/items/{itemId}")
operation GetItem {
    input := for Item {
        @required
        @httpLabel
        $listId

        @required
        @httpLabel
        $itemId

        @required
        @httpLabel
        $userId
    }

    output := for Item {
        @required
        $name

        @required
        $dueDate
    }

    errors: [
        ListeriaError
    ]
}

list Items {
    member: ItemSummary
}

structure ItemSummary for Item {
    $itemId
    $name
}

@readonly
@http(method: "GET", uri: "/users/{userId}/lists/{listId}/items")
operation ListItems {
    input := {
        @required
        @httpLabel
        listId: String

        @required
        @httpLabel
        userId: String
    }

    output := {
        @required
        items: Items
    }

    errors: [
        ListeriaError
    ]
}

$version: "2"

namespace common

string ItemId

resource Item {
    identifiers: {
        itemId: ItemId
        listId: ListId
        userId: UserId
    }
    properties: {
        name: String
        dueDate: Timestamp
    }
    read: GetItem
    list: ListItems
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
        listId: ListId

        @required
        @httpLabel
        userId: UserId
    }

    output := {
        @required
        items: Items
    }

    errors: [
        ListeriaError
    ]
}

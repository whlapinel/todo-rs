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
        complete: Boolean
        recurrence: String
        recurrenceBasis: String
        hasDueTime: Boolean
    }
    read: GetItem
    list: ListItems
    create: CreateItem
    update: UpdateItem
    delete: DeleteItem
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

        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime
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

        @required
        $complete

        $hasDueTime
    }

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/lists/{listId}/items/{itemId}")
operation UpdateItem {
    input := for Item {
        @required
        @httpLabel
        $userId

        @required
        @httpLabel
        $listId

        @required
        @httpLabel
        $itemId

        @required
        $name

        $dueDate

        @required
        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime
    }

    output := {}

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/lists/{listId}/items/{itemId}")
operation DeleteItem {
    input := for Item {
        @required
        @httpLabel
        $userId

        @required
        @httpLabel
        $listId

        @required
        @httpLabel
        $itemId
    }

    output := {}

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
    $dueDate
    $complete
    $recurrence
    $recurrenceBasis
    $hasDueTime
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

$version: "2"

namespace common

resource Item {
    identifiers: {
        itemId: String
        userId: String
    }
    properties: {
        name: String
        dueDate: Timestamp
        complete: Boolean
        recurrence: String
        recurrenceBasis: String
        hasDueTime: Boolean
        hasTasks: Boolean
        parentItemId: String
        hasChildren: Boolean
    }
    read: GetItem
    list: ListItems
    create: CreateItem
    update: UpdateItem
    delete: DeleteItem
}

@http(method: "POST", uri: "/users/{userId}/items")
operation CreateItem {
    input := for Item {
        @required
        @httpLabel
        $userId

        @required
        $name

        $dueDate

        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime

        $hasTasks

        $parentItemId

        @notProperty
        timezoneOffsetMinutes: Integer
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
@http(method: "GET", uri: "/users/{userId}/items/{itemId}")
operation GetItem {
    input := for Item {
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

        $hasTasks

        $parentItemId

        $hasChildren
    }

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/items/{itemId}")
operation UpdateItem {
    input := for Item {
        @required
        @httpLabel
        $userId

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

        $hasTasks

        $parentItemId

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := {}

    errors: [
        ListeriaError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/items/{itemId}")
operation DeleteItem {
    input := for Item {
        @required
        @httpLabel
        $userId

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
    $hasTasks
    $parentItemId
    $hasChildren
}

@input
structure ListItemsInput {
    @required
    @httpLabel
    userId: String

    @httpQuery("parentItemId")
    parentItemId: String
}

@output
structure ListItemsOutput {
    @required
    items: Items
}

@readonly
@http(method: "GET", uri: "/users/{userId}/items")
operation ListItems {
    input: ListItemsInput
    output: ListItemsOutput
    errors: [
        ListeriaError
    ]
}

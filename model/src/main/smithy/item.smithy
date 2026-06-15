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
        isTemplate: Boolean
        dueOffsetDays: Integer
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

        $dueOffsetDays

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := for Item {
        @required
        $itemId
    }

    errors: [
        PeoplesRepublicOfListsError
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

        $isTemplate

        $dueOffsetDays
    }

    errors: [
        PeoplesRepublicOfListsError
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

        $dueOffsetDays

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
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
        PeoplesRepublicOfListsError
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
    $isTemplate
    $dueOffsetDays
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
        PeoplesRepublicOfListsError
    ]
}

@http(method: "POST", uri: "/users/{userId}/templates")
operation CreateTemplate {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @notProperty
        name: String

        @notProperty
        sourceItemId: String
    }

    output := {
        @required
        @notProperty
        templateId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/templates")
operation ListTemplates {
    input := {
        @required
        @httpLabel
        userId: String
    }

    output := {
        @required
        @notProperty
        items: Items
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

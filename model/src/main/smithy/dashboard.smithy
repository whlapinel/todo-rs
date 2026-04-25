$version: "2"

namespace common

structure DueItemSummary {
    @required
    itemId: String

    @required
    listId: String

    @required
    listName: String

    @required
    name: String

    dueDate: Timestamp

    complete: Boolean

    recurrence: String

    recurrenceBasis: String

    hasDueTime: Boolean
}

list DueItems {
    member: DueItemSummary
}

@input
structure ListItemsDueInput {
    @required
    @httpLabel
    userId: String

    @notProperty
    @httpQuery("deadlineAfter")
    deadlineAfter: Timestamp

    @notProperty
    @httpQuery("deadlineBefore")
    deadlineBefore: Timestamp
}

@output
structure ListItemsDueOutput {
    @required
    @notProperty
    items: DueItems
}

@readonly
@http(method: "GET", uri: "/users/{userId}/items")
operation ListItemsDue {
    input: ListItemsDueInput
    output: ListItemsDueOutput
    errors: [
        ListeriaError
    ]
}

$version: "2"

namespace common

structure DueItemSummary {
    @required
    itemId: String

    @required
    name: String

    ownerUserId: String

    teamId: String

    assignedToUserId: String

    parentName: String

    dueDate: Timestamp

    scheduledDate: Timestamp

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
@http(method: "GET", uri: "/users/{userId}/due-items")
operation ListItemsDue {
    input: ListItemsDueInput
    output: ListItemsDueOutput
    errors: [
        PeoplesRepublicOfListsError
    ]
}

structure AssignedItemSummary {
    @required
    itemId: String

    @required
    name: String

    @required
    ownerUserId: String

    dueDate: Timestamp

    scheduledDate: Timestamp

    complete: Boolean

    recurrence: String

    recurrenceBasis: String

    hasDueTime: Boolean
}

list AssignedItems {
    member: AssignedItemSummary
}

@input
structure ListAssignedItemsInput {
    @required
    @httpLabel
    userId: String
}

@output
structure ListAssignedItemsOutput {
    @required
    @notProperty
    items: AssignedItems
}

@readonly
@http(method: "GET", uri: "/users/{userId}/assigned-items")
operation ListAssignedItems {
    input: ListAssignedItemsInput
    output: ListAssignedItemsOutput
    errors: [
        PeoplesRepublicOfListsError
    ]
}

$version: "2"

namespace common

enum ItemType {
    TASK
    EVENT
    TEMPLATE
    SIMPLE
}

list Items {
    member: ItemSummary
}

structure ItemSummary {
    itemId: String
    name: String
    description: String
    dueDate: Timestamp
    scheduledDate: Timestamp
    scheduledEndDate: Timestamp
    complete: Boolean
    recurrence: String
    recurrenceBasis: String
    hasDueTime: Boolean
    hasScheduledTime: Boolean
    hasEndTime: Boolean
    parentItemId: String
    hasChildren: Boolean
    itemType: ItemType
    eventType: String
    dueOffsetDays: Integer
    assignedToUserId: String
    sourceEventId: String
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
        description: String

        @notProperty
        sourceItemId: String

        @notProperty
        eventType: String
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

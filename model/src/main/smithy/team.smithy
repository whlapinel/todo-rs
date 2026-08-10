$version: "2"

namespace common

resource TeamItem {
    identifiers: {
        teamId: String
        itemId: String
    }
    properties: {
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
        points: Integer
    }
    read: GetTeamItem
    list: ListTeamItems
    create: CreateTeamItem
    update: UpdateTeamItem
    delete: DeleteTeamItem
}

@http(method: "POST", uri: "/teams/{teamId}/items")
operation CreateTeamItem {
    input := for TeamItem {
        @required
        @httpLabel
        $teamId

        @required
        $name

        $description

        $dueDate

        $scheduledDate

        $scheduledEndDate

        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime

        $hasScheduledTime

        $hasEndTime

        $parentItemId

        $itemType

        $eventType

        $dueOffsetDays

        $assignedToUserId

        $points

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := for TeamItem {
        @required
        $itemId
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/teams/{teamId}/items/{itemId}")
operation GetTeamItem {
    input := for TeamItem {
        @required
        @httpLabel
        $teamId

        @required
        @httpLabel
        $itemId
    }

    output := for TeamItem {
        @required
        $name

        $description

        $dueDate

        $scheduledDate

        $scheduledEndDate

        @required
        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime

        $hasScheduledTime

        $hasEndTime

        $parentItemId

        $hasChildren

        $itemType

        $eventType

        $dueOffsetDays

        $assignedToUserId

        $points
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/teams/{teamId}/items/{itemId}")
operation UpdateTeamItem {
    input := for TeamItem {
        @required
        @httpLabel
        $teamId

        @required
        @httpLabel
        $itemId

        @required
        $name

        $description

        $dueDate

        $scheduledDate

        $scheduledEndDate

        @required
        $complete

        $recurrence

        $recurrenceBasis

        $hasDueTime

        $hasScheduledTime

        $hasEndTime

        $parentItemId

        $itemType

        $eventType

        $dueOffsetDays

        $assignedToUserId

        $points

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/teams/{teamId}/items/{itemId}")
operation DeleteTeamItem {
    input := for TeamItem {
        @required
        @httpLabel
        $teamId

        @required
        @httpLabel
        $itemId
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

list TeamItems {
    member: TeamItemSummary
}

structure TeamItemSummary for TeamItem {
    $itemId
    $name
    $description
    $dueDate
    $scheduledDate
    $scheduledEndDate
    $complete
    $recurrence
    $recurrenceBasis
    $hasDueTime
    $hasScheduledTime
    $hasEndTime
    $parentItemId
    $hasChildren
    $itemType
    $eventType
    $dueOffsetDays
    $assignedToUserId
    assignedToUserName: String
    $points
}

@input
structure ListTeamItemsInput {
    @required
    @httpLabel
    teamId: String

    @httpQuery("parentItemId")
    parentItemId: String
}

@output
structure ListTeamItemsOutput {
    @required
    items: TeamItems
}

@readonly
@http(method: "GET", uri: "/teams/{teamId}/items")
operation ListTeamItems {
    input: ListTeamItemsInput
    output: ListTeamItemsOutput
    errors: [
        PeoplesRepublicOfListsError
    ]
}

@http(method: "POST", uri: "/teams/{teamId}/templates")
operation CreateTeamTemplate {
    input := {
        @required
        @httpLabel
        teamId: String

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
@http(method: "GET", uri: "/teams/{teamId}/templates")
operation ListTeamTemplates {
    input := {
        @required
        @httpLabel
        teamId: String
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

structure TeamMemberSummary {
    @required
    userId: String

    @required
    firstName: String

    @required
    lastName: String

    @required
    status: String

    @required
    role: String

    @required
    points: Integer
}

list TeamMembers {
    member: TeamMemberSummary
}

structure TeamSummary {
    @required
    teamId: String

    @required
    name: String

    @required
    status: String

    invitedByName: String
}

list Teams {
    member: TeamSummary
}

@http(method: "POST", uri: "/users/{userId}/teams")
operation CreateTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @notProperty
        name: String
    }

    output := {
        @required
        @notProperty
        teamId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams/{teamId}")
operation GetTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {
        @required
        @notProperty
        teamId: String

        @required
        @notProperty
        name: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/teams/{teamId}")
operation UpdateTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String

        @required
        @notProperty
        name: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams")
operation ListTeams {
    input := {
        @required
        @httpLabel
        userId: String
    }

    output := {
        @required
        @notProperty
        teams: Teams
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams/{teamId}/members")
operation ListTeamMembers {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {
        @required
        @notProperty
        members: TeamMembers
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@http(method: "POST", uri: "/users/{userId}/teams/{teamId}/invites")
operation InviteTeamMember {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String

        @required
        @notProperty
        inviteeUserId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/teams/{teamId}/accept")
operation AcceptTeamInvite {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/teams/{teamId}/members/{targetUserId}/role")
operation SetTeamMemberRole {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String

        @required
        @httpLabel
        @notProperty
        targetUserId: String

        @required
        @notProperty
        role: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/teams/{teamId}/membership")
operation LeaveTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

structure ActivityLogEntrySummary {
    @required
    entryId: String

    @required
    userId: String

    @required
    itemId: String

    @required
    itemName: String

    @required
    pointsDelta: Integer

    @required
    reversed: Boolean

    @required
    createdAt: Timestamp
}

list ActivityLogEntries {
    member: ActivityLogEntrySummary
}

@readonly
@http(method: "GET", uri: "/teams/{teamId}/activity-log")
operation ListTeamActivityLog {
    input := {
        @required
        @httpLabel
        teamId: String
    }

    output := {
        @required
        entries: ActivityLogEntries
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/teams/{teamId}/activity-log/{entryId}/undo")
operation UndoActivityLogEntry {
    input := {
        @required
        @httpLabel
        teamId: String

        @required
        @httpLabel
        entryId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

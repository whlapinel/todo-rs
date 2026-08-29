$version: "2"

namespace common

// Stage B4 (docs/project-abstraction-plan.md): the new unified item surface,
// used alongside the untouched legacy `Item`/`TeamItem` resources for the whole
// of Stage B. Modeled closely on `TeamItem` (team.smithy) — identifiers +
// properties + CRUD/list shape carried over verbatim, including
// assignedToUserId/points (a project can be team-backed, so both fields still
// need to exist on this surface even though they're inert for a team-less
// personal project — see `service::project_items`). Not attached to the
// service's `resources: [...]` list, same as `TeamItem` isn't either — its
// operations are registered directly in service.smithy's top-level
// `operations: [...]`.
resource ProjectItem {
    identifiers: {
        projectId: String
        itemId: String
    }
    properties: {
        name: String
        description: String
        dueDate: Timestamp
        scheduledDate: Timestamp
        scheduledEndDate: Timestamp
        complete: Boolean
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
        // Task-only, ungated (any project member may set it, no admin/team-backed
        // restriction — contrast with points/assignedToUserId above). See root
        // CLAUDE.md's Priority section.
        priority: Integer
        sourceEventId: String
        googleEventId: String
        calendarSubscriptionId: String
        // "Depends on" (docs/issues_and_features.md): the set of sibling Task items (same
        // project, same parentItemId — including both top-level, i.e. no parent) whose
        // completion gates this item's own completion. Settable only via UpdateProjectItem,
        // not CreateProjectItem — a freshly created item's siblings already exist, so there's
        // no ordering reason to force this onto the create path too; see
        // `service::item_dependencies` for the validation (Task-only, same project,
        // sibling-only, no self-reference, no cycles) and `StringList`'s own precedent
        // (item_series.smithy's rotationUserIds) for why a plain `list String` is enough here
        // — no dedicated shape needed. `None` on Update means "leave dependencies
        // unchanged" (deliberately not this model's usual direct-overwrite-Option
        // convention, since this is a side-table relation, not a plain Item column, and
        // every caller that isn't a dependency editor shouldn't have to round-trip the
        // current set just to touch an unrelated field) — pass an empty list to clear all.
        dependsOnItemIds: StringList
    }
    read: GetProjectItem
    list: ListProjectItems
    create: CreateProjectItem
    update: UpdateProjectItem
    delete: DeleteProjectItem
}

@http(method: "POST", uri: "/projects/{projectId}/items")
operation CreateProjectItem {
    input := for ProjectItem {
        @required
        @httpLabel
        $projectId

        @required
        $name

        $description

        $dueDate

        $scheduledDate

        $scheduledEndDate

        $complete

        $hasDueTime

        $hasScheduledTime

        $hasEndTime

        $parentItemId

        $itemType

        $eventType

        $dueOffsetDays

        $assignedToUserId

        $points

        $priority

        $sourceEventId

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := for ProjectItem {
        @required
        $itemId
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/items/{itemId}")
operation GetProjectItem {
    input := for ProjectItem {
        @required
        @httpLabel
        $projectId

        @required
        @httpLabel
        $itemId
    }

    output := for ProjectItem {
        @required
        $name

        $description

        $dueDate

        $scheduledDate

        $scheduledEndDate

        @required
        $complete

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

        $priority

        $sourceEventId

        $googleEventId

        $calendarSubscriptionId

        $dependsOnItemIds
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/items/{itemId}")
operation UpdateProjectItem {
    input := for ProjectItem {
        @required
        @httpLabel
        $projectId

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

        $hasDueTime

        $hasScheduledTime

        $hasEndTime

        $parentItemId

        $itemType

        $eventType

        $dueOffsetDays

        $assignedToUserId

        $points

        $priority

        $sourceEventId

        $dependsOnItemIds

        @notProperty
        timezoneOffsetMinutes: Integer
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/projects/{projectId}/items/{itemId}")
operation DeleteProjectItem {
    input := for ProjectItem {
        @required
        @httpLabel
        $projectId

        @required
        @httpLabel
        $itemId
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

list ProjectItems {
    member: ProjectItemSummary
}

structure ProjectItemSummary for ProjectItem {
    $itemId
    $name
    $description
    $dueDate
    $scheduledDate
    $scheduledEndDate
    $complete
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
    $priority
    $sourceEventId
    $googleEventId
    $calendarSubscriptionId
}

@input
structure ListProjectItemsInput {
    @required
    @httpLabel
    projectId: String

    @httpQuery("parentItemId")
    parentItemId: String
}

@output
structure ListProjectItemsOutput {
    @required
    items: ProjectItems
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/items")
operation ListProjectItems {
    input: ListProjectItemsInput
    output: ListProjectItemsOutput
    errors: [
        PeoplesRepublicOfListsError
    ]
}

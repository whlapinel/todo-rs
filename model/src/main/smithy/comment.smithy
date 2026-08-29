$version: "2"

namespace common

// "Add comments for tasks" (docs/issues_and_features.md): list + create only, no edit or
// delete. Plain operations (not a resource), scoped under
// /projects/{projectId}/items/{itemId}/comments — same "no userId path segment, acting
// user comes from the AuthUser extracted from the bearer token" precedent
// item_series.smithy documents. Any project member may comment on any Task item in that
// project (service::comments::create_comment enforces both the membership and the
// Task-only restriction); commenting on a virtual (not-yet-materialized) series
// occurrence is impossible by construction, since it has no itemId to attach to.
//
// authorUserId is the only identity carried on the wire — display-name resolution is a
// web_ui-side concern (mirrors how assignedToUserId is resolved to a name there already),
// not duplicated onto CommentSummary.
structure CommentSummary {
    @required
    commentId: String

    @required
    itemId: String

    @required
    projectId: String

    @required
    authorUserId: String

    @required
    body: String

    @required
    createdAt: Timestamp
}

list CommentList {
    member: CommentSummary
}

@http(method: "POST", uri: "/projects/{projectId}/items/{itemId}/comments")
operation CreateItemComment {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        itemId: String

        @required
        @notProperty
        body: String
    }

    output := {
        @required
        @notProperty
        commentId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/items/{itemId}/comments")
operation ListItemComments {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        itemId: String
    }

    output := {
        @required
        @notProperty
        comments: CommentList
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

$version: "2"

namespace common

// "Add comments for tasks" (docs/issues_and_features.md). Plain operations (not a
// resource), scoped under /projects/{projectId}/items/{itemId}/comments — same "no userId
// path segment, acting user comes from the AuthUser extracted from the bearer token"
// precedent item_series.smithy documents. Any project member may comment on any Task item
// in that project (service::comments::create_comment enforces both the membership and the
// Task-only restriction); commenting on a virtual (not-yet-materialized) series
// occurrence is impossible by construction, since it has no itemId to attach to.
//
// UpdateItemComment/DeleteItemComment (added when edit/delete were built, after the
// initial list+create-only version) are scoped one level deeper, under
// .../comments/{commentId}, and are author-only — service::comments::update_comment/
// delete_comment reject a non-author with ItemError::Invalid, the one place this model
// deviates from create_comment's/delete_attachment's ungated-among-members precedent
// (deliberate: rewriting or removing someone else's words is a different kind of action
// than deleting a shared file).
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

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/items/{itemId}/comments/{commentId}")
operation UpdateItemComment {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        itemId: String

        @required
        @httpLabel
        @notProperty
        commentId: String

        @required
        @notProperty
        body: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/projects/{projectId}/items/{itemId}/comments/{commentId}")
operation DeleteItemComment {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        itemId: String

        @required
        @httpLabel
        @notProperty
        commentId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

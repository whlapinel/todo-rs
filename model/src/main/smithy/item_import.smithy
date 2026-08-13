$version: "2"

namespace common

// CSV import (CLI/MCP only, no web UI): the server parses the raw CSV text itself
// (not the caller) so all parsing + validation + creation logic lives in one place,
// reusing `ProjectItem`'s own create path (`service::project_items::create_project_item`)
// per row. Not attached to the service's `resources: [...]` list, same as
// `ProjectItem`/`TeamItem` aren't either. `csv`/`format`/`results` aren't `ProjectItem`
// properties, so these operations use plain inline `input := { ... }`/`output := { ... }`
// structures rather than `for ProjectItem` mixin syntax.
@http(method: "POST", uri: "/projects/{projectId}/items/import")
operation ImportProjectItems {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        csv: String

        format: String

        timezoneOffsetMinutes: Integer
    }

    output := {
        @required
        results: ImportItemResults
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

list ImportItemResults {
    member: ImportItemResult
}

// `rowNumber` is 1-based counting the header as row 1, so the first data row is 2 —
// matches how a human counts lines opening the file in a spreadsheet/editor.
structure ImportItemResult {
    @required
    rowNumber: Integer

    @required
    success: Boolean

    itemId: String

    error: String
}

@readonly
@http(method: "GET", uri: "/items/import-template")
operation GetItemImportTemplate {
    input := {}

    output := {
        @required
        csv: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

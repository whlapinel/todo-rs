just created this file to document before I forget.

i've been considering adding calendar functionality to this app. there is a lot of overlap and typically these are separated completely but I think it's worth trying to integrate fully under a single app.

Some thoughts along these lines:
- Event types could be registered for ad hoc irregular events, even something occurring in the past, for the purpose of triggering instantiation of a checklist. So Checklists could be instantiated based on registered event types.
    - Kind of a bad Example but gives you the idea:
        - Event type: rained
        - Checklist "After rain": 
            - dump out standing water (due offset +1 day from event)
            - triggered by: event-type "rain"

- Events could be given a due date as well as a scheduled date. this makes them nearly indistinguishible from tasks doesn't it? so need to think more on this. maybe these are merge-able concepts, maybe not. what are the essential differences between what is entered in a calendar vs. a to-do list?

Separate note but related:
- Estimated duration of a task could be a new field, would help with scheduling and viewing in calendar
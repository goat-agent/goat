ALTER TABLE code_messages ADD COLUMN kind TEXT;

UPDATE code_messages
SET kind = 'wake'
WHERE role = 'user' AND body LIKE '%<environment-notice>%';

UPDATE code_messages
SET kind = 'plan_decision'
WHERE role = 'user'
  AND kind IS NULL
  AND (
      body LIKE '%The plan at % is approved%'
      OR body LIKE '%The user did not approve the plan%'
  );

UPDATE code_messages
SET kind = 'user'
WHERE role = 'user' AND kind IS NULL;

UPDATE code_messages
SET body = (
    SELECT json_group_array(value)
    FROM json_each(code_messages.body)
    WHERE COALESCE(json_extract(value, '$.text'), '') <> '[Reminder: write your prose to the user in the language they used in their request. Keep code, identifiers, file paths, shell commands, tool arguments, and quoted file or output excerpts exactly as they are. Text stored in the repository stays in the project''s prevailing language.]'
)
WHERE role = 'user' AND body LIKE '%[Reminder: write your prose%';

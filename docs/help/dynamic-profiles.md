# Dynamic Profiles

Dynamic Profiles let you define profiles in files instead of in Settings. You put a JSON or property list file in a watched folder. Termurion reads it, builds the profiles, and shows them in the Profiles menu and the profile list. When the file changes, the profiles change. When the file goes away, the profiles go away. Use this to generate profiles from a script, share them from a repository, or keep them under version control.

## The folder

By default the folder is:

```
%APPDATA%\Termurion\DynamicProfiles
```

To use another folder, set "Path to folder with dynamic profiles" in Settings (Ctrl+Shift+,), under Advanced. A `~` in the path means your home folder. Restart Termurion after you change it.

Termurion reads every file in the folder and its subfolders, up to 8 levels deep. It skips names that start with a dot or end with a tilde. It reloads shortly after a change. The delay is the Advanced setting "Delay between detecting a change to dynamic profiles and acting on it" (default 0.1 seconds, which lets a burst of changes count as one).

## A file

A file is JSON or a property list (XML or binary). The root must be a dictionary with a `Profiles` key that holds a list:

```json
{
  "Profiles": [
    {
      "Name": "Build Box",
      "Guid": "build-box-0001",
      "Dynamic Profile Parent Name": "Default",
      "Custom Command": "Yes",
      "Command": "wsl.exe -d Ubuntu",
      "Tags": ["work"]
    }
  ]
}
```

Rules for each profile:

- `Guid` is required. It must be 1 to 128 characters: letters, digits, and the characters `. _ { } -`. Keep it the same each time, or Termurion treats the profile as a new one.
- `Name` is required and cannot be empty.
- Any other key is a normal profile key. Keys you leave out come from the parent profile.
- A profile whose Guid matches one of your saved profiles is refused. A dynamic profile never replaces a saved one.

## Parent profiles

A dynamic profile starts from a parent and applies its own keys on top. Name the parent with either key:

- `Dynamic Profile Parent Name`: the name of another profile.
- `Dynamic Profile Parent GUID`: the Guid of another profile.

If you name neither, the parent is your default profile. If the parent you name does not exist, Termurion uses the default profile and reports an error.

## Tags

With the Advanced setting "Add a 'Dynamic' tag to dynamic profiles" on, each dynamic profile gets the tag `Dynamic`. Use it to filter the profile list.

## Editing a dynamic profile in Settings

A profile with `"Rewritable": true` can be edited in Settings. Termurion writes your change back into the same entry of the same file, and leaves the rest of the file alone. Keys the parent already supplies are not written. Without `Rewritable`, a dynamic profile is read only.

Termurion writes through a new hidden file in the same folder and renames it over the original. It refuses if the file changed on disk since it last read it.

## Safety

The folder is treated as untrusted input. Termurion loads a file only if all of these hold:

- You own the file and the folder. The only other accounts allowed to write are the system accounts (SYSTEM, Administrators, and TrustedInstaller).
- Nothing in the path is a symbolic link or junction, and the file has one name.
- The file has no Mark of the Web (a download zone of Internet or Restricted).

A file that fails these loads nothing. This matters because a profile can run a command when it opens.

Limits: a file can be 1 MiB, a folder 512 entries, 256 profiles in all, 512 keys in one profile, 64 KiB in any string, and 32 levels of nesting. A file past a limit is refused whole.

## Errors

A problem with a file, such as bad JSON or a missing Guid, shows an alert titled "Dynamic Profiles Error" with a button to reveal the folder. The details go to the log. Open it from Scripts > Manage > Console and choose the Dynamic Profiles entry. When a reload changes what a profile runs (its command, initial text, or triggers), the log says so.

## Related pages

- [Triggers](triggers.md)
- [Scripting Fundamentals](scripting-fundamentals.md)

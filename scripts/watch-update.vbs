Option Explicit

Dim shell, fileSystem, directory, command
Set shell = CreateObject("WScript.Shell")
Set fileSystem = CreateObject("Scripting.FileSystemObject")

directory = fileSystem.GetParentFolderName(WScript.ScriptFullName)
command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File """ & directory & "\watch-update.ps1"""

shell.Run command, 0, False

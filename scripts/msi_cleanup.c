/* GPL-3.0-only. Add uninstall-only rows to MSI's transactional registry action.
 * No direct registry writes: Windows Installer owns execution and rollback. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <msiquery.h>
#include <stdio.h>
#include <string.h>

__declspec(dllexport) UINT __stdcall AddOwnedAssociationCleanup(MSIHANDLE install)
{
    char extensions[4096], key[256], name[128], row_id[64];
    DWORD length = sizeof(extensions);
    UINT result = MsiGetPropertyA(install, "SUI_OPTIONAL_EXTENSIONS", extensions, &length);
    MSIHANDLE database = 0, view = 0, record = 0;
    char *cursor = NULL, *extension;
    unsigned index = 0;
    if (result != ERROR_SUCCESS) return result;
    if (!extensions[0]) return ERROR_SUCCESS;
    database = MsiGetActiveDatabase(install);
    if (!database) return ERROR_INSTALL_FAILURE;
    result = MsiDatabaseOpenViewA(database,
        "SELECT `Registry`, `Root`, `Key`, `Name`, `Value`, `Component_` FROM `Registry`", &view);
    if (result != ERROR_SUCCESS) goto done;
    result = MsiViewExecute(view, 0);
    if (result != ERROR_SUCCESS) goto done;
    for (extension = strtok_s(extensions, ";", &cursor); extension;
         extension = strtok_s(NULL, ";", &cursor)) {
        const char *c;
        if (extension[0] != '.' || !extension[1] || strlen(extension) > 32) {
            result = ERROR_INSTALL_FAILURE; goto done;
        }
        for (c = extension + 1; *c; ++c) {
            if (!((*c >= 'a' && *c <= 'z') || (*c >= '0' && *c <= '9') || *c == '_')) {
                result = ERROR_INSTALL_FAILURE; goto done;
            }
        }
        sprintf_s(key, sizeof(key), "Software\\Classes\\%s\\OpenWithProgids", extension);
        sprintf_s(name, sizeof(name), "SuiSuiView.Installed%s", extension);
        sprintf_s(row_id, sizeof(row_id), "UninstallOptional%u", index++);
        record = MsiCreateRecord(6);
        if (!record) { result = ERROR_INSTALL_FAILURE; goto done; }
        if ((result = MsiRecordSetStringA(record, 1, row_id)) != ERROR_SUCCESS ||
            (result = MsiRecordSetInteger(record, 2, 1)) != ERROR_SUCCESS ||
            (result = MsiRecordSetStringA(record, 3, key)) != ERROR_SUCCESS ||
            (result = MsiRecordSetStringA(record, 4, name)) != ERROR_SUCCESS ||
            (result = MsiRecordSetStringA(record, 6, "Registration")) != ERROR_SUCCESS) goto done;
        result = MsiViewModify(view, MSIMODIFY_INSERT_TEMPORARY, record);
        MsiCloseHandle(record); record = 0;
        if (result != ERROR_SUCCESS) goto done;
    }
done:
    if (record) MsiCloseHandle(record);
    if (view) MsiCloseHandle(view);
    MsiCloseHandle(database);
    return result;
}

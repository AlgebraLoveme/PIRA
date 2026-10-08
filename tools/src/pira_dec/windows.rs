//! Native Windows privacy boundary. Existing security descriptors are never rewritten.
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, Prefix};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation,
    INHERIT_ONLY_ACE, IsValidAcl, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS, READ_CONTROL,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

// TOKEN_USER includes a pointer into the buffer. Keep its aligned storage alive.
struct User(Vec<usize>);
impl User {
    fn current() -> io::Result<Self> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0;
        let result = unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed)
        };
        if result != 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return Err(io::Error::other("cannot size Windows token user"));
        }
        if needed < size_of::<TOKEN_USER>() as u32 || needed > 65536 {
            return Err(io::Error::other("invalid Windows token user size"));
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = Self(buffer);
        if unsafe { IsValidSid(user.sid()) } == 0 {
            return Err(io::Error::other("invalid Windows token user SID"));
        }
        Ok(user)
    }

    fn sid(&self) -> PSID {
        unsafe { (*(self.0.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }

    fn descriptor(&self) -> io::Result<LocalAllocation> {
        let mut text = null_mut();
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _text = LocalAllocation(text.cast());
        let mut len = 0;
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) })
            .map_err(|_| io::Error::other("invalid Windows SID text"))?;
        // Protected DACL: no permissive parent ACL is inherited during creation.
        // Only the process user is granted access; privileged actors can bypass ACLs.
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalAllocation(descriptor))
    }
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    // Normalize with the OS path routine, not lossy Unicode conversion or
    // filesystem canonicalization. Native long paths need the verbatim prefix.
    let path = std::path::absolute(path)?;
    let mut encoded: Vec<u16> = path.as_os_str().encode_wide().collect();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in Windows path",
        ));
    }
    if encoded.len() >= 248 {
        match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(_) => encoded = r"\\?\".encode_utf16().chain(encoded).collect(),
                Prefix::UNC(_, _) => {
                    encoded = r"\\?\UNC\"
                        .encode_utf16()
                        .chain(encoded.into_iter().skip(2))
                        .collect()
                }
                _ => {} // Already verbatim paths keep their native representation.
            },
            _ => return Err(io::Error::other("Windows path is not absolute")),
        }
    }
    encoded.push(0);
    Ok(encoded)
}

fn security_attributes(descriptor: &LocalAllocation) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

pub fn create_directory(path: &Path) -> io::Result<()> {
    let user = User::current()?;
    let descriptor = user.descriptor()?;
    if unsafe { CreateDirectoryW(wide(path)?.as_ptr(), &security_attributes(&descriptor)) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn create_new_file(path: &Path) -> io::Result<File> {
    open_private_file(path, CREATE_NEW)
}

pub fn open_lock(path: &Path) -> io::Result<File> {
    // OPEN_ALWAYS applies the descriptor only to new objects, never existing ones.
    open_private_file(path, OPEN_ALWAYS)
}

fn open_private_file(path: &Path, disposition: u32) -> io::Result<File> {
    let user = User::current()?;
    let descriptor = user.descriptor()?;
    let handle = unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security_attributes(&descriptor),
            disposition,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub fn check_private_file(file: &File) -> Result<(), String> {
    check_private(file).map_err(|error| format!("validate Windows private storage/output: {error}"))
}

fn trusted_sid(sid: PSID, user: &User) -> bool {
    unsafe {
        EqualSid(sid, user.sid()) != 0
            || IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
    }
}

fn check_private(file: &File) -> io::Result<()> {
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("refusing Windows reparse point"));
    }
    let user = User::current()?;
    let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut::<ACL>(), null_mut());
    let error = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _descriptor = LocalAllocation(descriptor);
    if owner.is_null() || unsafe { IsValidSid(owner) } == 0 || !trusted_sid(owner, &user) {
        return Err(io::Error::other(
            "object owner is neither the current process user nor a privileged system/administrator principal",
        ));
    }
    if dacl.is_null() || unsafe { IsValidAcl(dacl) } == 0 {
        return Err(io::Error::other("missing or invalid private DACL"));
    }
    // Only ordinary allow/deny ACEs are supported. Unknown callback/object forms
    // are not approximated. GetAce and IsValidAcl establish the entry boundaries.
    for index in 0..u32::from(unsafe { (*dacl).AceCount }) {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        match header.AceType {
            1 => continue, // ACCESS_DENIED_ACE_TYPE
            0 => {}        // ACCESS_ALLOWED_ACE_TYPE
            _ => return Err(io::Error::other("unsupported private DACL ACE type")),
        }
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), usize::from(header.AceSize)) };
        // Ordinary allow ACE: header(4), mask(4), SID(8 + 4*SubAuthorityCount).
        if bytes.len() < 16 || bytes.len() < 16 + 4 * usize::from(bytes[9]) {
            return Err(io::Error::other("truncated private DACL allow ACE"));
        }
        let mask = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let sid = unsafe { ace.cast::<u8>().add(8) }.cast();
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(io::Error::other("invalid private DACL SID"));
        }
        if mask == 0 || trusted_sid(sid, &user) {
            continue;
        }
        return Err(io::Error::other(
            "DACL grants access beyond the current user, SYSTEM and Administrators; existing ACLs are never rewritten",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use windows_sys::Win32::Security::Authorization::SetSecurityInfo;
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pira-dec-windows-{}", crate::util::nonce_hex()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn acl_editor(path: &Path) -> File {
        OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }

    // Only synthetic test fixtures have their ACLs changed.
    fn set_dacl(file: &File, sddl: &str) {
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            },
            0
        );
        let _descriptor = LocalAllocation(descriptor);
        let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
            },
            0
        );
        assert_ne!(present, 0);
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null_mut(),
                )
            },
            0
        );
    }

    #[test]
    fn private_creation_blocks_permissive_inheritance_and_never_overwrites() {
        let sandbox = Sandbox::new();
        let parent = sandbox.0.join("permissive");
        create_directory(&parent).unwrap();
        set_dacl(&acl_editor(&parent), "D:P(A;OICI;FA;;;WD)");
        assert!(check_private_file(&open_directory(&parent).unwrap()).is_err());
        let child = parent.join("private");
        create_directory(&child).unwrap();
        check_private_file(&open_directory(&child).unwrap()).unwrap();
        let output = child.join("export");
        crate::util::write_private_new(&output, b"private bytes").unwrap();
        check_private_file(&File::open(&output).unwrap()).unwrap();
        assert!(crate::util::write_private_new(&output, b"replacement").is_err());
        assert_eq!(fs::read(&output).unwrap(), b"private bytes");
    }

    #[test]
    fn private_creation_preserves_long_path_support() {
        let sandbox = Sandbox::new();
        let first = sandbox.0.join("a".repeat(120));
        create_directory(&first).unwrap();
        let second = first.join("b".repeat(120));
        create_directory(&second).unwrap();
        let output = second.join("record");
        crate::util::write_private_new(&output, b"long path").unwrap();
        check_private_file(&File::open(&output).unwrap()).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"long path");
    }

    #[test]
    fn existing_dacl_validation_accepts_restrictions_and_rejects_broad_or_null_grants() {
        let sandbox = Sandbox::new();
        let path = sandbox.0.join("record");
        let mut file = create_new_file(&path).unwrap();
        file.write_all(b"unchanged").unwrap();
        file.sync_all().unwrap();
        let editor = acl_editor(&path);
        for sddl in ["D:P(D;;FW;;;WD)", "D:P(A;;FA;;;SY)(A;;FA;;;BA)"] {
            set_dacl(&editor, sddl);
            check_private_file(&file).unwrap();
        }
        for sddl in ["D:P(A;;FA;;;WD)", "D:NO_ACCESS_CONTROL"] {
            set_dacl(&editor, sddl);
            assert!(check_private_file(&file).unwrap_err().contains("DACL"));
            // Validation did not silently repair the object.
            assert!(check_private_file(&file).is_err());
        }
        assert_eq!(fs::read(&path).unwrap(), b"unchanged");
    }
}

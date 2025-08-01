use std::env::{Vars, VarsOs};

pub use sys_traits::{
    BaseEnvSetCurrentDir, BaseEnvSetVar, BaseEnvVar, BaseFsCanonicalize,
    BaseFsChown, BaseFsCloneFile, BaseFsCopy, BaseFsCreateDir,
    BaseFsCreateJunction, BaseFsHardLink, BaseFsMetadata, BaseFsOpen,
    BaseFsRead, BaseFsReadDir, BaseFsReadLink, BaseFsRemoveDir,
    BaseFsRemoveDirAll, BaseFsRemoveFile, BaseFsRename, BaseFsSetFileTimes,
    BaseFsSetPermissions, BaseFsSetSymlinkFileTimes, BaseFsSymlinkChown,
    BaseFsSymlinkDir, BaseFsSymlinkFile, BaseFsWrite, BoxableFsFile,
    CreateDirOptions, EnvCacheDir, EnvCurrentDir, EnvHomeDir, EnvProgramsDir,
    EnvSetCurrentDir, EnvSetUmask, EnvSetVar, EnvTempDir, EnvUmask, EnvVar,
    FileType, FsCanonicalize, FsChown, FsCloneFile, FsCopy, FsCreateDir,
    FsCreateDirAll, FsCreateJunction, FsDirEntry, FsFile, FsFileAsRaw,
    FsFileIsTerminal, FsFileLock, FsFileLockMode, FsFileMetadata, FsFileSetLen,
    FsFileSetPermissions, FsFileSetTimes, FsFileSyncAll, FsFileSyncData,
    FsFileTimes, FsHardLink, FsMetadata, FsOpen, FsRead, FsReadDir, FsReadLink,
    FsRemoveDir, FsRemoveDirAll, FsRemoveFile, FsRename, FsSetFileTimes,
    FsSetPermissions, FsSetSymlinkFileTimes, FsSymlinkChown, FsSymlinkDir,
    FsSymlinkFile, FsWrite, SystemRandom, SystemTimeNow, ThreadSleep,
};

pub trait EnvVars {
    fn env_vars(&self) -> Vars;

    fn env_vars_os(&self) -> VarsOs;
}

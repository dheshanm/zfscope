//! Built-in help, written for people new to ZFS: what a setting or number means, why it matters, typical values
//! and the gotchas. Condensed from zfsprops(7), zpoolprops(7), zpool-features(7) and zfs(4) (OpenZFS 2.4).
//!
//! Each entry is (summary, detail). The detail may contain blank-line paragraph breaks.

pub type Help = (&'static str, &'static str);

pub fn dataset_prop(name: &str) -> Option<Help> {
    Some(match name {
        // ---------------------------------------------------------------- space accounting (read-only)
        "used" => (
            "Total disk space this dataset takes up, including its snapshots and everything below it.",
            "used = the dataset's own data + its snapshots + its children + any unused reservation (each part is \
             shown separately in the usedby* properties).\n\nIt is the physical size after compression, so it can be \
             much smaller than the files' apparent size (see logicalused). Destroying the dataset frees roughly this much.",
        ),
        "available" | "avail" => (
            "How much more data can be written here before something stops you.",
            "Takes into account free space in the pool and any quota or reservation on this dataset or its parents. \
             Several datasets often show the same 'available' because they share the same free pool space: writing \
             to one reduces it for all of them.",
        ),
        "referenced" | "refer" => (
            "Size of the data you would see if you looked at this dataset right now.",
            "Counts every block the live filesystem points to, even blocks also held by snapshots or shared with \
             clones. That is why deleting a file may not reduce 'used': if a snapshot still holds the blocks, they \
             stay on disk until the snapshot is destroyed.",
        ),
        "usedbydataset" => {
            ("Space that would be freed by destroying this dataset's live data (not its snapshots or children).", "")
        }
        "usedbysnapshots" => (
            "Space held only by snapshots: freed if you destroyed ALL snapshots of this dataset.",
            "Snapshots keep old versions of changed or deleted data. This number grows as the live data changes.\n\n\
             Note: it is NOT the sum of the snapshots' own 'used' values. A block shared by two snapshots belongs to \
             neither alone, so it shows up here but in no single snapshot's 'used'.",
        ),
        "usedbychildren" => ("Space used by the datasets nested below this one (and their snapshots).", ""),
        "usedbyrefreservation" => (
            "Space set aside by refreservation that hasn't been written yet.",
            "It counts as used so that nobody else can take it.",
        ),
        "logicalused" => (
            "How big everything in 'used' would be without compression.",
            "Compare with 'used' to see how much compression is saving across this dataset and its children.",
        ),
        "logicalreferenced" => ("How big 'referenced' would be without compression.", ""),
        "compressratio" => (
            "How well data in this dataset (and below) compresses: logical size ÷ physical size.",
            "1.00x means no savings (data is already compressed, like video or archives). 1.5–3x is common for text, \
             logs, databases and VM images. Only data written after compression was enabled is compressed.",
        ),
        "refcompressratio" => ("Compression ratio of 'referenced' (this dataset's live data only).", ""),
        "written" => (
            "Data written since the most recent snapshot.",
            "This is roughly how big the next snapshot would be, and how much an incremental `zfs send` from the \
             last snapshot would transfer.",
        ),
        "snapshots_changed" => ("When a snapshot of this dataset was last created, destroyed or renamed.", ""),
        "createtxg" => (
            "The transaction group (TXG) in which the dataset was created.",
            "TXGs are ZFS's internal timeline (see the ZIL·TXG tab), so this orders datasets and snapshots by \
             creation time.",
        ),
        "creation" => ("When the dataset was created.", ""),
        "guid" => (
            "A unique 64-bit ID for this dataset.",
            "It stays the same through zfs send/receive, which is how ZFS recognises that two snapshots on \
             different machines are the same one.",
        ),
        "objsetid" => (
            "Internal object-set number of the dataset.",
            "zfscope uses it to match /proc/spl/kstat/zfs/<pool>/objset-0x<id>, which provides the per-dataset IO \
             and ZIL numbers in the Datasets tab.",
        ),
        "type" => ("filesystem, volume (a block device, zvol), snapshot or bookmark.", ""),
        "origin" => (
            "For a clone: the snapshot it was created from.",
            "A clone starts out sharing all its data with that snapshot, so the snapshot cannot be destroyed while \
             the clone exists. `zfs promote` reverses the relationship.",
        ),
        "clones" => ("For a snapshot: datasets that were cloned from it.", ""),
        "userrefs" => (
            "Number of 'holds' on this snapshot.",
            "A held snapshot cannot be destroyed (zfs hold / zfs release). Replication tools use holds to keep the \
             snapshot they need for the next incremental send.",
        ),
        "defer_destroy" => (
            "This snapshot has been marked for destruction (zfs destroy -d).",
            "It goes away automatically as soon as its last hold or clone is gone.",
        ),
        "mounted" => ("Whether the filesystem is currently mounted.", ""),
        "filesystem_count" | "snapshot_count" => (
            "Number of filesystems / snapshots at and below this dataset.",
            "Only tracked when a filesystem_limit or snapshot_limit is set somewhere above.",
        ),
        "receive_resume_token" => (
            "Set when a resumable `zfs receive -s` was interrupted.",
            "Pass it to `zfs send -t <token>` to continue the transfer where it stopped instead of starting over.",
        ),
        "keystatus" => (
            "Whether the encryption key is loaded (available) or not (unavailable).",
            "An encrypted dataset can only be mounted and read while its key is loaded (zfs load-key).",
        ),
        "encryptionroot" => (
            "The dataset whose key this encrypted dataset uses.",
            "Children inherit their parent's key. Loading, unloading or changing the key happens at the encryption \
             root and affects every dataset under it.",
        ),
        "redact_snaps" => ("Snapshots this redacted dataset was redacted against (zfs send --redact).", ""),
        "version" => ("On-disk format version of the filesystem layer (ZPL). 5 is current.", ""),
        "utf8only" => ("Reject file names that aren't valid UTF-8. Fixed when the dataset is created.", ""),
        "normalization" => (
            "Unicode normalization applied when comparing file names (none, formC, formD, formKC, formKD).",
            "Makes visually identical names (é as one or two code points) match. Fixed at creation; needed for \
             predictable behaviour with macOS/SMB clients.",
        ),
        "casesensitivity" => (
            "Whether File.txt and file.txt are different files (sensitive) or the same (insensitive, mixed).",
            "Fixed at creation. 'insensitive' is common for SMB shares serving Windows clients.",
        ),
        // ---------------------------------------------------------------- quotas & reservations
        "quota" => (
            "Hard limit on space for this dataset AND everything below it, snapshots included.",
            "When the limit is reached, writes fail with 'disk quota exceeded'. Because snapshots count, a busy \
             dataset with many snapshots can hit its quota even though users deleted files.\n\nnone = no limit. \
             Tip: refquota is often a better fit for user-facing limits.",
        ),
        "refquota" => (
            "Hard limit on this dataset's own live data (snapshots and children don't count).",
            "Usually the better choice for a home directory or project limit: users get exactly what they see, and \
             snapshot growth doesn't eat into it.",
        ),
        "reservation" => (
            "Space guaranteed to this dataset and everything below it.",
            "The reserved space is subtracted from what other datasets can use, even if it isn't filled yet. Useful \
             to make sure an important dataset can never be starved by another one filling the pool.",
        ),
        "refreservation" => (
            "Space guaranteed for this dataset's own live data (not snapshots).",
            "Volumes (zvols) get refreservation = volsize by default ('thick provisioning'), so the VM disk can \
             never run out of space. Set it to none for a thin (sparse) volume.",
        ),
        "filesystem_limit" | "snapshot_limit" => (
            "Maximum number of filesystems / snapshots that can be created at and below this dataset.",
            "Protects against runaway scripts. none = unlimited.",
        ),
        "userquota" | "groupquota" | "projectquota" => ("Per user / group / project space limit.", ""),
        "defaultuserquota"
        | "defaultgroupquota"
        | "defaultprojectquota"
        | "defaultuserobjquota"
        | "defaultgroupobjquota"
        | "defaultprojectobjquota" => (
            "Default space (or file-count) limit for every user / group / project without its own quota.",
            "0 = no default limit.",
        ),
        // ---------------------------------------------------------------- data layout & performance
        "recordsize" => (
            "The largest block a file is split into (default 128K).",
            "ZFS stores, checksums and compresses each block as a unit, and rewrites a whole block when any part \
             of it changes.\n\nLarge (1M): best for big files read or written sequentially (media, backups, \
             archives): better compression, less metadata, faster streaming.\nSmall (8K–64K): best for databases \
             and VM images doing small random reads/writes, so a 8K update doesn't rewrite a 1M block.\n\nTip: match \
             it to the application's IO size. Only affects files written after the change.",
        ),
        "volblocksize" => (
            "Block size of a volume (zvol). Fixed when the volume is created (default 16K).",
            "Match it to the IO size of whatever uses the volume (a VM's filesystem, a database). Too small wastes \
             space on metadata and RAIDZ padding; too large makes small writes expensive.",
        ),
        "volsize" => ("Size of the volume (zvol) as seen by whatever uses it.", ""),
        "compression" => (
            "Compress data as it is written: off, lz4, zstd, zstd-N, gzip-N, zle, on (= lz4).",
            "lz4 is fast enough to be almost free and gives up quickly on incompressible data: a good default \
             nearly everywhere. zstd compresses better at more CPU cost (zstd-1…19). Compression also speeds up \
             disks, because fewer bytes are read and written.\n\nOnly new writes are affected; existing data stays \
             as it was written.",
        ),
        "checksum" => (
            "How data blocks are checksummed: on (fletcher4), sha256, sha512, blake3, skein, edonr, off.",
            "Every block is verified on read, and with redundancy (mirror, raidz) a bad copy is repaired \
             automatically: this is how ZFS catches silent disk corruption. Stronger hashes are needed for dedup.\n\n\
             Never use 'off' — you lose ZFS's ability to detect and heal corruption.",
        ),
        "dedup" => (
            "Store identical blocks only once (deduplication).",
            "Every write looks up a dedup table (DDT), which costs RAM and IO; when the table doesn't fit in memory, \
             performance can collapse. Only worth it for data with many exact duplicates (many identical VM images). \
             For copying files, block cloning (copy_file_range) is usually a cheaper way to share blocks.",
        ),
        "copies" => (
            "Keep 1, 2 or 3 copies of every data block, on top of mirror/raidz redundancy.",
            "Protects against small damaged areas (e.g. on a single-disk laptop pool), not against losing a whole \
             disk. Multiplies space used. Only affects new writes.",
        ),
        "sync" => (
            "What happens when an application asks for its data to be safely on disk (fsync, O_SYNC, NFS COMMIT).",
            "standard: honour the request via the ZFS Intent Log (ZIL) — correct and the default.\nalways: treat \
             every write as synchronous (safe but slow without a fast SLOG device).\ndisabled: say 'done' before \
             the data is on disk. Fast, but a crash or power loss loses the last few seconds of acknowledged \
             writes — databases and NFS clients may end up corrupted. Avoid unless the data is disposable.",
        ),
        "logbias" => (
            "How synchronous writes use the log: latency (default) or throughput.",
            "latency sends sync writes through the SLOG (separate log device) for the fastest acknowledgement. \
             throughput writes large sync writes straight to the main pool and keeps only a pointer in the log, \
             which avoids saturating the SLOG with big streaming writes (e.g. databases with their own logs).",
        ),
        "primarycache" => (
            "What this dataset may keep in the ARC (the RAM cache): all, metadata or none.",
            "metadata is useful for data that is read once (backups, streaming) or already cached by the \
             application (databases with their own buffer pool), so it doesn't push more useful data out of RAM.",
        ),
        "secondarycache" => ("What this dataset may keep in the L2ARC (cache devices): all, metadata or none.", ""),
        "special_small_blocks" => (
            "Blocks up to this size go to the special vdev (fast SSDs) instead of the main disks.",
            "The special vdev always holds metadata; this adds small file blocks too, which speeds up workloads with \
             many small files.\n\nCareful: if this is ≥ recordsize, ALL data of the dataset goes to the special \
             vdev and can fill it up. 0 = metadata only.",
        ),
        "redundant_metadata" => (
            "How many extra copies of metadata ZFS keeps: all (default), most, some, none.",
            "Extra metadata copies protect the filesystem structure; lower settings reduce write overhead for \
             workloads with lots of metadata updates.",
        ),
        "dnodesize" => (
            "Size of the per-file metadata record (dnode): legacy (512 bytes), auto, 1k–16k.",
            "Larger dnodes (auto) let extended attributes and ACLs live inside the dnode with xattr=sa, saving an \
             extra disk read for each file. auto is recommended on Linux with xattr=sa.",
        ),
        "xattr" => (
            "Where extended attributes are stored: sa (inside the file's metadata, fast) or dir (hidden directories).",
            "sa is much faster for workloads that use xattrs (SELinux labels, ACLs, Samba). on means sa on Linux.",
        ),
        "atime" => (
            "Update a file's 'last accessed' time whenever it is read.",
            "Turning it off avoids a metadata write for every read, which helps read-heavy workloads. relatime=on \
             is a compromise that updates atime at most once a day.",
        ),
        "relatime" => (
            "Update atime only when it is older than the modification time or more than 24h old.",
            "Keeps tools that rely on atime (mail readers, tmp cleaners) working with far fewer writes.",
        ),
        "prefetch" => (
            "Read ahead when ZFS detects sequential access: all, metadata or none.",
            "Prefetch speeds up streaming reads; disabling it can help purely random workloads.",
        ),
        "direct" => (
            "How O_DIRECT reads/writes are handled: standard, always, disabled.",
            "Direct IO (2.3+) bypasses the ARC for properly aligned requests, useful for applications that do their \
             own caching.",
        ),
        "nopwrite" => ("Skip rewriting blocks whose content hasn't changed (needs a strong checksum).", ""),
        "volmode" => (
            "How a volume is exposed to the system: default, full, dev, none.",
            "dev exposes the block device without scanning it for partitions; none hides it.",
        ),
        "volthreading" => ("Process IO to this volume on multiple threads.", ""),
        "snapdev" => ("Whether snapshots of a volume appear as read-only block devices (hidden / visible).", ""),
        // ---------------------------------------------------------------- mounting & sharing
        "mountpoint" => (
            "Where the filesystem appears in the directory tree.",
            "By default it follows the dataset name (/pool/a/b) and children inherit it. legacy = mounted via \
             /etc/fstab instead; none = never mounted.",
        ),
        "canmount" => (
            "Whether the filesystem can be mounted: on, off, noauto.",
            "off is used for 'container' datasets that only exist to pass properties down to their children. \
             noauto mounts only when you ask explicitly.",
        ),
        "readonly" => ("Make the filesystem read-only (no changes possible through it).", ""),
        "exec" => ("Allow running programs stored on this filesystem.", ""),
        "setuid" => ("Honour setuid/setgid bits on programs stored here.", ""),
        "devices" => ("Allow device files (/dev nodes) on this filesystem to work.", ""),
        "overlay" => ("Allow mounting on top of a directory that isn't empty.", ""),
        "zoned" => ("The dataset is managed from inside a container / user namespace.", ""),
        "sharenfs" => (
            "Share this filesystem over NFS: off, on, or exportfs options (e.g. rw=@10.0.0.0/24,no_root_squash).",
            "ZFS writes the export to /etc/exports.d/zfs.exports and children inherit it.",
        ),
        "sharesmb" => ("Share this filesystem over SMB (Windows file sharing) via Samba.", ""),
        "snapdir" => (
            "Visibility of the .zfs directory at the top of the filesystem: hidden, visible, disabled.",
            "Even when hidden you can cd into .zfs/snapshot/<name> to browse a snapshot read-only — handy for \
             users restoring their own files.",
        ),
        "aclmode" => ("What chmod does to an existing ACL (access control list).", ""),
        "aclinherit" => ("How ACL entries are inherited by new files and directories.", ""),
        "acltype" => (
            "Kind of ACLs supported: off, posix (Linux getfacl/setfacl), nfsv4.",
            "posix with xattr=sa is the usual choice on Linux.",
        ),
        "context" | "fscontext" | "defcontext" | "rootcontext" => {
            ("SELinux security contexts applied when mounting.", "")
        }
        "vscan" | "nbmand" | "mlslabel" => {
            ("Legacy Solaris options (virus scanning, mandatory locks, security labels). Not used on Linux.", "")
        }
        "longname" => ("Allow file names longer than 255 bytes (up to 1023).", ""),
        // ---------------------------------------------------------------- encryption
        "encryption" => (
            "Native encryption algorithm (off, aes-256-gcm…). Chosen when the dataset is created.",
            "Encrypts file data and most metadata. Dataset names, sizes and properties remain visible.",
        ),
        "keyformat" => ("Format of the encryption key: passphrase, hex or raw.", ""),
        "keylocation" => ("Where the key is read from: prompt (ask), file:///path, or https://.", ""),
        "pbkdf2iters" => (
            "How many rounds turn a passphrase into a key.",
            "Higher is slower to brute-force; it only affects how long loading the key takes.",
        ),
        _ if name.contains(':') => (
            "A user property (namespace:name). ZFS stores it but doesn't act on it.",
            "Used by snapshot and replication tools (com.sun:auto-snapshot, sanoid, syncoid) and for your own notes \
             and labels. Inherited by children like native properties.",
        ),
        _ => return None,
    })
}

pub fn pool_prop(name: &str) -> Option<Help> {
    if let Some(f) = name.strip_prefix("feature@") {
        return Some(feature(f));
    }
    Some(match name {
        "size" => (
            "Total raw size of the pool's main storage.",
            "For raidz this includes the space used by parity, so it is larger than what you can actually store. \
             `zfs list` 'avail' shows usable space.",
        ),
        "allocated" => ("Raw space in use, including raidz parity.", ""),
        "free" => (
            "Raw unallocated space (parity not subtracted).",
            "For the space you can actually still write, look at 'available' on the pool's top dataset.",
        ),
        "capacity" => (
            "Percentage of the pool that is allocated.",
            "ZFS needs free space to write efficiently (it never overwrites in place). Above ~80% writes get \
             slower as free space becomes scarce and fragmented; plan to add capacity before ~85–90%.",
        ),
        "fragmentation" => (
            "How chopped-up the FREE space is (0% = large free regions, 100% = tiny scattered holes).",
            "This is not file fragmentation. High values mean new writes must be pieced together from small holes, \
             which slows writes. It rises with capacity and churn.",
        ),
        "health" => (
            "Overall pool state: ONLINE, DEGRADED, FAULTED, OFFLINE, REMOVED or UNAVAIL.",
            "DEGRADED = a device failed but redundancy keeps data available — replace it soon. FAULTED/UNAVAIL = \
             data may be inaccessible.",
        ),
        "dedupratio" => ("How much deduplication is saving (1.00x = nothing).", ""),
        "dedup_table_size" => ("On-disk size of the dedup table (DDT).", ""),
        "dedup_table_quota" => ("Upper limit for the dedup table's size (auto = based on the dedup device).", ""),
        "expandsize" => (
            "Extra space available after growing devices (zpool online -e).",
            "Appears when disks were replaced with bigger ones but the pool hasn't expanded into them yet.",
        ),
        "checkpoint" => (
            "Space held by a pool checkpoint (zpool checkpoint).",
            "A checkpoint lets you rewind the entire pool, but it pins everything freed since — discard it \
             (zpool checkpoint -d) once you no longer need it.",
        ),
        "freeing" => ("Space still being freed in the background after deleting large datasets or snapshots.", ""),
        "leaked" => ("Space lost during background frees. Should be 0.", ""),
        "ashift" => (
            "Sector size the pool assumes when creating new vdevs, as a power of two (12 = 4 KiB).",
            "Fixed per vdev at creation. Too small for disks with 4K sectors (most modern disks and SSDs) causes \
             slow read-modify-write cycles; 12 is the safe default.",
        ),
        "autoexpand" => ("Grow the pool automatically when its disks are replaced with larger ones.", ""),
        "autoreplace" => ("Automatically use a new disk inserted in the same physical slot as a failed one.", ""),
        "autotrim" => (
            "Tell SSDs which blocks are free (TRIM) continuously.",
            "Keeps SSD write performance steady. The alternative is running `zpool trim` periodically.",
        ),
        "bootfs" => ("Dataset the system boots from (for root-on-ZFS systems).", ""),
        "cachefile" => ("File where the pool's configuration is remembered so it's imported at boot.", ""),
        "comment" => ("Free-text comment stored on the disks themselves.", ""),
        "compatibility" => (
            "Restrict which features may be enabled, for portability to other systems or older ZFS.",
            "e.g. openzfs-2.1-linux keeps the pool importable by OpenZFS 2.1.",
        ),
        "delegation" => ("Allow non-root users to be given permissions with `zfs allow`.", ""),
        "failmode" => (
            "What happens if the pool loses so many devices it can't continue: wait, continue, panic.",
            "wait (default) blocks IO until the devices return; continue returns errors to new writes; panic \
             crashes the machine.",
        ),
        "listsnapshots" => ("Show snapshots in `zfs list` even without -t snapshot.", ""),
        "multihost" => (
            "Protection against importing the pool on two machines at once (for shared storage).",
            "Only needed when several hosts can see the same disks.",
        ),
        "readonly" => ("The pool was imported read-only.", ""),
        "version" => ("Legacy version number; '-' means the pool uses feature flags instead (normal).", ""),
        "guid" | "load_guid" => ("Unique identifier of the pool.", ""),
        "altroot" => ("Temporary prefix added to all mountpoints (set when importing, e.g. for rescue).", ""),
        "bcloneused" => ("Space shared through block cloning (cp --reflink, copy_file_range).", ""),
        "bclonesaved" => ("Space saved by block cloning compared with real copies.", ""),
        "bcloneratio" => ("How much block cloning is saving (1.00x = nothing).", ""),
        "last_scrubbed_txg" => ("Last transaction group covered by a completed scrub.", ""),
        _ if name.contains(':') => ("A user property; ZFS stores it but doesn't act on it.", ""),
        _ => return None,
    })
}

/// Pool feature flags (zpool-features(7)).
/// Appended to every feature flag's help.
pub const FEATURE_STATE: &str = "enabled = available but not used yet; active = in use on disk, so the pool now needs \
                                 a ZFS version that understands it (older versions can no longer import it).";

pub fn feature(name: &str) -> Help {
    match name {
        "async_destroy" => (
            "Destroy large datasets in the background.",
            "The space comes back gradually; the pool's 'freeing' property shows how much is left.",
        ),
        "empty_bpobj" => {
            ("Less work when snapshots hold no unique data.", "Speeds up creating and destroying snapshots.")
        }
        "lz4_compress" => ("Allows compression=lz4.", ""),
        "zstd_compress" => ("Allows compression=zstd (better ratio than lz4 at more CPU).", ""),
        "multi_vdev_crash_dump" => ("Crash dumps on pools with several vdevs (illumos). Unused on Linux.", ""),
        "spacemap_histogram" => (
            "Track the size of free regions.",
            "Makes the 'fragmentation' figure possible and helps the allocator pick good spots.",
        ),
        "spacemap_v2" => ("More compact free-space records.", "Less metadata to read and write on large pools."),
        "log_spacemap" => (
            "Batch free-space updates in a per-pool log.",
            "Fewer metadata writes per transaction group: faster TXG syncs on big pools.",
        ),
        "enabled_txg" => ("Remember when each feature was enabled.", "Needed by other features."),
        "hole_birth" => (
            "Remember when holes (sparse regions) were created.",
            "Makes incremental zfs send correct and efficient for sparse files.",
        ),
        "extensible_dataset" => ("Datasets can carry new kinds of metadata.", "Building block for other features."),
        "embedded_data" => (
            "Store tiny blocks inside the block pointer itself.",
            "Very small files and blocks take no separate disk space and no extra IO.",
        ),
        "bookmarks" => (
            "zfs bookmark: remember a snapshot's point in time without keeping the snapshot.",
            "Lets you destroy old snapshots but still send incrementals from them.",
        ),
        "bookmark_v2" => ("Bookmarks for encrypted and redacted data.", ""),
        "bookmark_written" => {
            ("Bookmarks record how much was written since them.", "Enables the written#bookmark property.")
        }
        "redaction_bookmarks" | "redacted_datasets" | "redaction_list_spill" => (
            "Redacted sends (zfs send --redact).",
            "Send a copy of a dataset with chosen data left out, e.g. to share a sanitised copy.",
        ),
        "filesystem_limits" => ("Allows the filesystem_limit and snapshot_limit properties.", ""),
        "large_blocks" => ("Allows recordsize above 128K (up to 16M).", "Useful for large sequential files."),
        "large_dnode" => (
            "Allows dnodesize above 512 bytes.",
            "Lets extended attributes live inside the file's metadata (faster with xattr=sa).",
        ),
        "large_microzap" => {
            ("Bigger 'micro' directory blocks.", "Makes very large directories cheaper when large blocks are in use.")
        }
        "longname" => ("File names up to 1023 bytes.", ""),
        "sha512" => ("Allows checksum=sha512.", ""),
        "skein" => ("Allows checksum=skein.", ""),
        "edonr" => ("Allows checksum=edonr.", ""),
        "blake3" => ("Allows checksum=blake3 (fast cryptographic checksum).", ""),
        "userobj_accounting" => (
            "Count files per user and group.",
            "Enables file-count quotas (userobjquota) and `zfs userspace` object counts.",
        ),
        "project_quota" => ("Quotas per project ID (a directory tree), not just per user/group.", ""),
        "encryption" => ("Native per-dataset encryption.", ""),
        "device_removal" => (
            "Remove a top-level vdev from the pool (zpool remove).",
            "Its data is copied elsewhere and a small mapping table remains.",
        ),
        "obsolete_counts" => (
            "Track how much of a removed device's mapping is no longer needed.",
            "Lets ZFS shrink the leftover mapping over time.",
        ),
        "zpool_checkpoint" => {
            ("zpool checkpoint: a whole-pool 'undo point'.", "Useful before risky operations such as upgrades.")
        }
        "allocation_classes" => (
            "Special and dedup vdevs.",
            "Lets metadata and small blocks live on fast SSDs (the 'special' class) while data stays on HDDs.",
        ),
        "resilver_defer" => (
            "Don't restart a running resilver when another disk needs one.",
            "The second resilver waits instead, so the first finishes sooner.",
        ),
        "device_rebuild" => (
            "Sequential rebuild (zpool replace -s, dRAID).",
            "Restores redundancy faster by copying in disk order, then verifies with a scrub.",
        ),
        "livelist" => ("Faster clone deletion.", ""),
        "draid" => (
            "dRAID vdevs: RAID with distributed spare space.",
            "Rebuilds after a disk failure much faster on large pools.",
        ),
        "raidz_expansion" => ("Add a disk to an existing raidz vdev (zpool attach).", ""),
        "zilsaxattr" => (
            "Log extended-attribute changes (xattr=sa) in the ZIL.",
            "Makes xattr updates crash-safe for synchronous workloads.",
        ),
        "head_errlog" => (
            "Track data errors per filesystem rather than pool-wide.",
            "zpool status -v can then report affected files in snapshots and clones accurately.",
        ),
        "block_cloning" | "block_cloning_endian" => (
            "Block cloning: copies that share data blocks (cp --reflink, copy_file_range).",
            "A cloned file takes no extra space until it is modified. See bcloneused / bclonesaved.",
        ),
        "vdev_zaps_v2" | "avz_v2" => {
            ("Per-vdev properties storage.", "Lets `zpool get` / `zpool set` work on individual vdevs.")
        }
        "fast_dedup" => (
            "New, faster deduplication table format (OpenZFS 2.3).",
            "Much lower overhead than classic dedup, with table size limits.",
        ),
        "dynamic_gang_header" => {
            ("Larger 'gang block' headers.", "Less overhead when a nearly full pool must split blocks into pieces.")
        }
        "physical_rewrite" => (
            "zfs rewrite -P: rewrite data in place while keeping its logical birth time.",
            "Lets existing data pick up new settings (e.g. compression, special vdev) without disturbing incremental sends.",
        ),
        _ => ("A pool feature flag.", "See `man zpool-features` for details."),
    }
}

pub fn param(name: &str) -> Option<Help> {
    Some(match name {
        // ARC
        "zfs_arc_max" => (
            "Largest size the ARC (ZFS's RAM cache) may grow to, in bytes.",
            "0 = automatic (most of RAM on OpenZFS 2.3+). The ARC gives memory back under pressure, but slowly; \
             lower this if other applications need guaranteed memory.",
        ),
        "zfs_arc_min" => ("Smallest size the ARC shrinks to under memory pressure (0 = automatic).", ""),
        "zfs_arc_meta_balance" => (
            "How strongly the ARC prefers keeping metadata over file data (higher = more metadata).",
            "Metadata (directory and block-pointer information) is what makes finding files fast.",
        ),
        "zfs_arc_dnode_limit_percent" => {
            ("Share of the ARC that file metadata (dnodes) may use before being evicted.", "")
        }
        "zfs_arc_sys_free" => ("Memory the ARC tries to leave free for the rest of the system.", ""),
        "zfs_arc_shrinker_limit" => ("Maximum memory the ARC releases each time the kernel asks for some back.", ""),
        "l2arc_write_max" => ("How fast the L2ARC (SSD cache) is filled, bytes per interval.", ""),
        "l2arc_noprefetch" => ("Don't put prefetched (streaming) data into the L2ARC.", ""),
        "zfs_prefetch_disable" => {
            ("Turn off read-ahead (prefetch).", "Can help purely random workloads; usually hurts sequential reads.")
        }
        // TXG / dirty data / write throttle
        "zfs_txg_timeout" => (
            "Maximum seconds between transaction-group commits (default 5).",
            "Writes collect in memory and are committed to disk as a group (TXG). A TXG is committed after this \
             many seconds, or sooner when enough dirty data has piled up.",
        ),
        "zfs_dirty_data_max" => (
            "How much written-but-not-yet-on-disk ('dirty') data may be held in memory.",
            "More absorbs bigger write bursts but each TXG takes longer to commit. When it fills, writers are slowed \
             down (the write throttle). Default: 10% of RAM, capped at 4 GiB.",
        ),
        "zfs_dirty_data_max_percent" => ("zfs_dirty_data_max as a percentage of RAM (when not set directly).", ""),
        "zfs_dirty_data_max_max" => ("Upper limit for zfs_dirty_data_max.", ""),
        "zfs_dirty_data_sync_percent" => {
            ("Start committing a TXG early once dirty data reaches this % of zfs_dirty_data_max.", "")
        }
        "zfs_delay_min_dirty_percent" => (
            "Start slowing down writers when dirty data passes this % of zfs_dirty_data_max.",
            "If the 'delayed/s' counter in the write throttle panel climbs, the disks can't keep up with incoming \
             writes and applications are being made to wait.",
        ),
        "zfs_delay_scale" => ("How steeply writers are slowed as dirty data approaches the limit.", ""),
        "zfs_sync_taskq_batch_pct" => ("Share of CPUs used for committing TXGs.", ""),
        // vdev IO scheduler
        "zfs_vdev_async_write_max_active" => (
            "Max background writes in flight per disk while committing a TXG.",
            "Higher values push more IO at the disks at once. HDD pools with slow TXG syncs sometimes benefit from \
             raising it; too high hurts read latency during syncs.",
        ),
        "zfs_vdev_async_write_min_active" => ("Min background writes in flight per disk while committing a TXG.", ""),
        "zfs_vdev_async_write_active_min_dirty_percent" | "zfs_vdev_async_write_active_max_dirty_percent" => {
            ("Dirty-data range over which background write concurrency ramps from min to max.", "")
        }
        "zfs_vdev_async_read_max_active" | "zfs_vdev_async_read_min_active" => {
            ("Concurrent background (prefetch) reads per disk.", "")
        }
        "zfs_vdev_sync_read_max_active" | "zfs_vdev_sync_read_min_active" => {
            ("Concurrent reads an application is waiting for, per disk.", "")
        }
        "zfs_vdev_sync_write_max_active" | "zfs_vdev_sync_write_min_active" => {
            ("Concurrent synchronous (ZIL) writes per disk.", "")
        }
        "zfs_vdev_max_active" => ("Total IOs in flight per disk, across all kinds.", ""),
        "zfs_vdev_scrub_max_active" | "zfs_vdev_scrub_min_active" => ("Concurrent scrub/resilver IOs per disk.", ""),
        "zfs_vdev_aggregation_limit" => (
            "Largest IO formed by merging neighbouring requests.",
            "See the 'agg' bars in the request-size histogram (Pool tab).",
        ),
        "zfs_vdev_aggregation_limit_non_rotating" => ("Same limit for SSDs.", ""),
        "zfs_vdev_queue_depth_pct" => ("Write queue depth per disk, % of zfs_vdev_async_write_max_active.", ""),
        "zio_slow_io_ms" => (
            "An IO taking longer than this (ms) counts as a 'slow I/O' in zpool status.",
            "Slow I/Os on one disk often point to a failing drive or a bad cable/path.",
        ),
        "zfs_deadman_synctime_ms" | "zfs_deadman_ziotime_ms" => {
            ("How long a TXG commit / single IO may hang before ZFS reports it ('deadman').", "")
        }
        // ZIL / SLOG
        "zfs_immediate_write_sz" => (
            "Synchronous writes larger than this are written straight to the pool; the log keeps only a pointer.",
            "Avoids copying big writes twice (once into the log, once into the pool).",
        ),
        "zil_slog_bulk" => (
            "Above this many bytes per commit, sync writes stop getting priority on the SLOG.",
            "Keeps one big writer from crowding out small latency-sensitive ones.",
        ),
        "zfs_commit_timeout_pct" => {
            ("How long a log commit waits to collect more writes, % of recent log latency.", "")
        }
        "zil_maxblocksize" => ("Largest log block written to the ZIL.", ""),
        "zil_nocacheflush" => {
            ("Don't ask log devices to flush their write caches.", "Only safe with power-loss-protected SSDs.")
        }
        "zfs_nocacheflush" => {
            ("Don't ask any device to flush its write cache.", "Unsafe unless every device has power-loss protection.")
        }
        // scrub / resilver
        "zfs_scan_vdev_limit" => ("Max data in flight per disk during scrub/resilver.", ""),
        "zfs_resilver_min_time_ms" => (
            "Minimum time per TXG spent on resilvering (rebuilding a replaced disk).",
            "Raise it to resilver faster at the expense of normal IO.",
        ),
        "zfs_scrub_min_time_ms" => ("Minimum time per TXG spent on scrubbing.", ""),
        "zfs_scan_mem_lim_fact" => ("Fraction of RAM a scrub may use to sort IO.", ""),
        "zfs_scan_legacy" => ("Use the old unsorted scrub (slower on HDDs).", ""),
        // misc
        "zfs_compressed_arc_enabled" => ("Keep data compressed in the ARC so more fits in RAM.", ""),
        "zfs_abd_scatter_enabled" => ("Use non-contiguous memory for cached data.", ""),
        "spa_slop_shift" => (
            "Space reserved for ZFS itself: 1/2^N of the pool (default 5 = 3.2%).",
            "Keeps room for metadata and frees, so a 'full' pool can still delete files.",
        ),
        "metaslab_lba_weighting_enabled" => ("Prefer the faster outer part of HDDs when allocating.", ""),
        "zfs_special_class_metadata_reserve_pct" => ("Keep this % of the special vdev for metadata only.", ""),
        "zfs_dmu_offset_next_sync" => ("Make sparse-file hole detection accurate by syncing first.", ""),
        "zfs_bclone_enabled" => ("Enable block cloning (copy_file_range / cp --reflink).", ""),
        "zfs_dio_enabled" => ("Enable Direct IO support.", ""),
        "zfs_txg_history" => {
            ("How many recent TXGs are kept in /proc/spl/kstat/zfs/<pool>/txgs (source of the TXG table).", "")
        }
        "zfs_read_history" => ("Entries kept in the per-pool reads kstat.", ""),
        _ => return None,
    })
}

/// Parameters worth showing next to the TXG/ZIL view.
pub const TXG_PARAMS: [&str; 10] = [
    "zfs_txg_timeout",
    "zfs_dirty_data_max",
    "zfs_dirty_data_sync_percent",
    "zfs_delay_min_dirty_percent",
    "zfs_vdev_async_write_max_active",
    "zfs_vdev_async_write_min_active",
    "zfs_vdev_sync_write_max_active",
    "zfs_immediate_write_sz",
    "zil_slog_bulk",
    "zio_slow_io_ms",
];

pub const ARC_PARAMS: [&str; 6] = [
    "zfs_arc_max",
    "zfs_arc_min",
    "zfs_arc_meta_balance",
    "zfs_arc_sys_free",
    "zfs_prefetch_disable",
    "l2arc_write_max",
];

// ------------------------------------------------------------------------------------------------ view guides

/// "What am I looking at": a short guide per tab, shown by `?` above the key list.
/// (Guide text spans source lines; render with `words()`.)
pub fn tab_guide(tab: crate::app::Tab) -> &'static [(&'static str, &'static str)] {
    use crate::app::Tab;
    match tab {
        Tab::Overview => &[
            (
                "Pool card",
                "Health, how full each storage class is (data disks, special = fast metadata SSDs, logs = \
              SLOG) and how fragmented its free space is, the last scrub, and errors. Keep capacity below ~80%.",
            ),
            (
                "IOPS / bandwidth",
                "Operations and bytes per second for the whole pool: reads rise above the axis, writes \
              fall below it, on the same scale. Write spikes every few seconds are normal: that's ZFS committing a \
              transaction group (TXG).",
            ),
            (
                "Latency",
                "How long IOs take end-to-end ('total wait' = waiting in ZFS's queues + time on the disk). \
              The title also shows the disk part alone. HDDs typically sit at a few ms; tens of ms means queueing.",
            ),
            (
                "Cards",
                "ARC = ZFS's RAM cache (hit % = reads served from memory). ZIL = log of synchronous writes. TXG = \
              how often and how long data is committed to disk. Busiest datasets = who is doing the IO right now.",
            ),
        ],
        Tab::Pool => &[
            (
                "Topology",
                "The pool's structure: the pool is made of vdevs (raidz groups, mirrors, special, log) made of \
              disks. R/W/CK count read, write and checksum errors per device: any non-zero CK means data was found \
              corrupt (and repaired if there was redundancy).",
            ),
            (
                "Columns (m)",
                "IO: operations and bytes per second. latency: total = queue + disk time; syncq/asyncq = \
              time waiting in ZFS's queues for reads/writes an app waits for (sync) vs background work (async). \
              queues: IOs waiting (pend) and on the disk (act). space: size, used, fragmentation.",
            ),
            (
                "Heatmap",
                "Latency over time for the selected row: each column is 5 s, each row a latency bucket, colour = \
              how many IOs. A healthy disk shows a tight band; a struggling one smears upward.",
            ),
            (
                "Request sizes",
                "Sizes of IOs sent to the device. ind = single requests, agg = neighbouring requests \
              merged into one larger IO.",
            ),
            (
                "Disk grid (d)",
                "Every disk as a tile, coloured by the chosen metric, so one slow or erroring disk \
              among many stands out.",
            ),
            ("History (H)", "zpool history: every command run against the pool, and internal events."),
        ],
        Tab::Datasets => &[
            (
                "Datasets",
                "Filesystems and volumes (zvols) in a tree. USED includes snapshots and children; REFER is the \
              live data you see; SNAP is space held only by snapshots; RATIO is compression.",
            ),
            (
                "Share of parent",
                "How much of the parent's space this dataset takes, split into its own data (█), \
              snapshots (▓) and children (▒).",
            ),
            (
                "ztop (t)",
                "A flat list of datasets by live IO: reads/writes per second, file deletions (unlink) and \
              synchronous-write activity (ZIL).",
            ),
            ("Detail (Enter)", "Space breakdown and quotas, IO and ZIL graphs, the snapshot list, and all properties."),
        ],
        Tab::Arc => &[
            (
                "ARC",
                "Adaptive Replacement Cache: ZFS keeps recently (MRU) and frequently (MFU) used blocks in RAM. A \
              high hit % means most reads never touch the disks.",
            ),
            (
                "Hit ratios",
                "demand = data an application asked for; prefetch = data ZFS read ahead expecting it to be \
              needed. iohits are reads that found the block already being fetched.",
            ),
            (
                "Ghost hits",
                "Reads for blocks that were recently evicted. Many ghost hits suggest more RAM (or an L2ARC) \
              would help.",
            ),
            (
                "Lists",
                "How the cache is split between recent (MRU) and frequent (MFU) data, and how much of it could \
              be evicted if memory were needed.",
            ),
        ],
        Tab::Zil => &[
            (
                "ZIL",
                "The ZFS Intent Log records synchronous writes (fsync, databases, NFS) so they can be acknowledged \
              quickly and replayed after a crash. With a SLOG (fast log SSD) the log lives there; otherwise on the \
              main disks.",
            ),
            (
                "Log record kinds",
                "copied/needcopy: the data itself goes into the log. indirect: the data is written to \
              the pool and the log only keeps a pointer (large writes).",
            ),
            (
                "TXG",
                "Transaction groups: ZFS collects writes in memory and commits them as a batch every few seconds. \
              Sync time should stay well below zfs_txg_timeout; if it doesn't, the disks can't keep up.",
            ),
            (
                "Write throttle",
                "When too much un-committed (dirty) data piles up, ZFS slows down writers. Non-zero \
              'delayed/s' means applications are being held back by disk speed.",
            ),
        ],
        Tab::Disks => &[
            (
                "Disks",
                "Every physical device behind the pool, with the slot it sits in, how many paths reach it \
              (multipath), model and serial, and SMART health from the drive itself.",
            ),
            (
                "Health",
                "SMART ok/warn/FAIL is the drive's own verdict. DEFECT = grown defects (bad sectors the drive \
              remapped); ERR = uncorrected errors. Rising numbers mean the drive should be replaced.",
            ),
            (
                "Load",
                "UTIL and AWAIT come from the kernel (how busy the device is, average time per IO); ZFS WAIT is \
              ZFS's view of the same disk. One disk much slower than its siblings is a classic sign of trouble.",
            ),
        ],
        Tab::Props => &[
            (
                "Properties",
                "Every setting of a pool or dataset. 'settable' ones can be changed with zfs set / zpool \
              set; 'read-only' ones are measurements or were fixed at creation.",
            ),
            (
                "Source",
                "local = set on this dataset; inherited from X = set on a parent; default = never set; received \
              = came with zfs receive.",
            ),
            (
                "Feature flags",
                "Optional on-disk capabilities of a pool. 'active' features are in use and require a \
              ZFS version that understands them.",
            ),
        ],
    }
}

/// A few core terms, shown at the end of every tab guide.
pub const GLOSSARY: &[(&str, &str)] = &[
    ("pool", "a set of disks managed as one storage space"),
    ("vdev", "a group of disks giving redundancy (mirror, raidz1/2/3, dRAID); pools stripe data across vdevs"),
    ("dataset", "a filesystem or volume inside a pool, with its own properties"),
    ("snapshot", "a read-only point-in-time copy; costs space only as data changes"),
    ("ARC / L2ARC", "RAM cache / optional SSD cache"),
    ("ZIL / SLOG", "log for synchronous writes / a fast device dedicated to it"),
    ("TXG", "transaction group: a batch of writes committed together every few seconds"),
    ("scrub / resilver", "read and verify everything / rebuild a replaced disk"),
];

// ------------------------------------------------------------------------------------------------ choices

/// Possible values of a property, each with what it does / who it suits: (value, description).
pub type Opts = &'static [(&'static str, &'static str)];

const ON_OFF: Opts = &[("on", "enabled"), ("off", "disabled")];

pub fn dataset_options(name: &str) -> Option<Opts> {
    Some(match name {
        "compression" => &[
            ("off", "no compression; only for data that is already compressed or encrypted by the application"),
            ("on", "= lz4 (the recommended default)"),
            ("lz4", "very fast, gives up early on incompressible data; good default almost everywhere"),
            ("zstd", "= zstd-3: noticeably better ratio than lz4 at moderate CPU cost; good for logs, text, backups"),
            ("zstd-N", "zstd level 1–19: higher = smaller but slower writes (reads stay fast); 1–3 for general use"),
            ("zstd-fast-N", "faster than lz4-class speed at a lower ratio; for very high write rates"),
            ("gzip-N", "gzip level 1–9: good ratio but slow; zstd is usually better at the same ratio"),
            ("zle", "only squeezes runs of zeros; nearly free"),
            ("lzjb", "legacy algorithm; use lz4 instead"),
        ],
        "checksum" => &[
            ("on", "= fletcher4: fast and detects corruption; the right choice unless you dedup"),
            ("fletcher4", "fast non-cryptographic checksum (default)"),
            ("sha256", "cryptographic; needed for dedup/nopwrite; slower"),
            ("sha512", "cryptographic, faster than sha256 on 64-bit CPUs"),
            ("blake3", "cryptographic and very fast on modern CPUs; good choice for dedup"),
            ("skein", "cryptographic, fast"),
            ("edonr", "very fast cryptographic hash (needs verify for dedup)"),
            ("off", "NO corruption detection or self-healing — never recommended"),
        ],
        "dedup" => &[
            ("off", "no deduplication (default; right for almost everyone)"),
            ("on", "deduplicate with sha256; costs RAM/IO for the dedup table on every write"),
            ("verify", "like on, but byte-compares blocks before sharing them"),
            ("sha256", "dedup with sha256 checksums"),
            ("sha512", "dedup with sha512 checksums"),
            ("blake3", "dedup with blake3 checksums (fast)"),
            ("skein", "dedup with skein checksums"),
            ("edonr,verify", "dedup with edonr (must verify)"),
        ],
        "sync" => &[
            ("standard", "honour fsync/O_SYNC via the ZIL: safe and the default"),
            ("always", "every write is synchronous: safest, slow without a fast SLOG"),
            ("disabled", "acknowledge before data is on disk: fast but loses recent writes on power loss"),
        ],
        "logbias" => &[
            ("latency", "sync writes go through the SLOG for the fastest acknowledgement (default)"),
            (
                "throughput",
                "large sync writes go straight to the pool; avoids saturating the SLOG (databases, streaming)",
            ),
        ],
        "primarycache" | "secondarycache" => &[
            ("all", "cache data and metadata (default)"),
            ("metadata", "cache only metadata; for data read once or cached by the application"),
            ("none", "cache nothing for this dataset"),
        ],
        "prefetch" => &[
            ("all", "read ahead data and metadata (default)"),
            ("metadata", "read ahead metadata only"),
            ("none", "no read-ahead; for purely random workloads"),
        ],
        "redundant_metadata" => &[
            ("all", "extra copies of all metadata (default, safest)"),
            ("most", "skip extra copies of the lowest-level metadata; fewer writes for small-block workloads"),
            ("some", "extra copies only of critical metadata"),
            ("none", "no extra copies beyond the pool's redundancy"),
        ],
        "dnodesize" => &[
            ("legacy", "512-byte dnodes; for compatibility with old systems"),
            ("auto", "grow as needed so xattrs fit inside (recommended with xattr=sa)"),
            ("1k", "fixed 1K dnodes"),
            ("2k", "fixed 2K dnodes"),
            ("4k", "fixed 4K dnodes"),
            ("8k", "fixed 8K dnodes"),
            ("16k", "fixed 16K dnodes"),
        ],
        "xattr" => &[
            ("sa", "store xattrs inside the file's metadata: fastest (SELinux, ACLs, Samba)"),
            ("on", "= sa on Linux"),
            ("dir", "store xattrs in hidden directories: slower, maximum compatibility"),
            ("off", "no extended attributes"),
        ],
        "atime" => &[
            ("on", "update access time on every read (extra metadata writes)"),
            ("off", "never update access time: best for read-heavy workloads"),
        ],
        "relatime" => &[
            ("on", "update access time at most daily; keeps atime-dependent tools working cheaply"),
            ("off", "atime follows the atime property exactly"),
        ],
        "snapdir" => &[
            ("hidden", ".zfs exists but isn't listed (default); users can still cd into it"),
            ("visible", ".zfs is listed in the top directory"),
            ("disabled", "no .zfs directory at all"),
        ],
        "canmount" => &[
            ("on", "mounted automatically (default)"),
            ("noauto", "mounted only when explicitly asked"),
            ("off", "never mounted; a container that only passes properties to its children"),
        ],
        "aclmode" => &[
            ("discard", "chmod discards ACL entries that the mode doesn't express (default)"),
            ("groupmask", "chmod reduces ACL permissions to fit the group bits"),
            ("passthrough", "chmod leaves the ACL alone"),
            ("restricted", "chmod fails on files with non-trivial ACLs"),
        ],
        "aclinherit" => &[
            ("discard", "new files inherit no ACL entries"),
            ("noallow", "inherit only deny entries"),
            ("restricted", "inherit entries minus write_acl/write_owner (default)"),
            ("passthrough", "inherit entries unchanged"),
            ("passthrough-x", "like passthrough, but execute only where the mode asks"),
        ],
        "acltype" => &[
            ("off", "no ACLs, just Unix permissions"),
            ("posix", "POSIX ACLs (getfacl/setfacl); the usual choice on Linux"),
            ("nfsv4", "NFSv4-style ACLs"),
        ],
        "casesensitivity" => &[
            ("sensitive", "File.txt and file.txt are different (Unix behaviour)"),
            ("insensitive", "they are the same file (Windows/SMB behaviour)"),
            ("mixed", "case-preserving, insensitive lookups for SMB clients"),
        ],
        "normalization" => &[
            ("none", "compare names byte for byte"),
            ("formC", "compose characters (é as one code point) before comparing"),
            ("formD", "decompose characters before comparing (macOS style)"),
            ("formKC", "compatibility composition"),
            ("formKD", "compatibility decomposition"),
        ],
        "copies" => &[
            ("1", "one copy, protected by the pool's redundancy (default)"),
            ("2", "two copies of every block: survives small damaged areas, doubles space"),
            ("3", "three copies: for irreplaceable data on a single disk"),
        ],
        "volmode" => &[
            ("default", "use the system default (full)"),
            ("full", "expose the volume and scan it for partitions"),
            ("dev", "expose the block device without partition scanning (VM disks)"),
            ("none", "don't expose the volume at all"),
        ],
        "snapdev" => &[
            ("hidden", "snapshots of the volume aren't visible as devices"),
            ("visible", "each snapshot appears as a read-only device"),
        ],
        "direct" => &[
            ("standard", "honour O_DIRECT for aligned requests (bypass the ARC)"),
            ("always", "treat all aligned IO as direct"),
            ("disabled", "ignore O_DIRECT; everything goes through the ARC"),
        ],
        "encryption" => &[
            ("off", "not encrypted"),
            ("on", "= aes-256-gcm"),
            ("aes-256-gcm", "default when on; fast on CPUs with AES instructions"),
            ("aes-128-gcm", "smaller key, marginally faster"),
            ("aes-256-ccm", "CCM mode; slower than GCM"),
        ],
        "keyformat" => &[
            ("passphrase", "a typed passphrase, stretched with PBKDF2"),
            ("hex", "a 64-character hex key"),
            ("raw", "a 32-byte binary key file"),
        ],
        "recordsize" => &[
            ("4K", "tiny random IO; rarely a good idea (metadata overhead)"),
            ("8K", "PostgreSQL / Oracle data files"),
            ("16K", "MySQL/InnoDB pages, some VM images"),
            ("32K", "small-file mixes"),
            ("64K", "VM images, SQL Server, mixed workloads"),
            ("128K", "default: general purpose filesystems"),
            ("256K", "larger files, mostly sequential"),
            ("512K", "big sequential files"),
            ("1M", "media, backups, archives, large sequential files: best compression"),
            ("4M", "very large streaming files (needs large_blocks)"),
            ("16M", "maximum; very large sequential files only"),
        ],
        "volblocksize" => &[
            ("4K", "matches 4K-sector guests; high metadata overhead"),
            ("8K", "databases on zvols"),
            ("16K", "default: VM disks"),
            ("32K", "VMs with larger IO"),
            ("64K", "VMs with mostly sequential IO"),
            ("128K", "large sequential IO (backup targets)"),
        ],
        "special_small_blocks" => &[
            ("0", "only metadata goes to the special vdev (default)"),
            ("4K", "tiny blocks too"),
            ("16K", "small files and small tails"),
            ("32K", "common choice for many-small-files workloads"),
            ("64K", "more data on SSD; watch special vdev capacity"),
            ("128K", "with recordsize 128K this sends ALL data to the special vdev"),
        ],
        "readonly" | "exec" | "setuid" | "devices" | "overlay" | "zoned" | "utf8only" | "vscan" | "nbmand"
        | "volthreading" | "longname" => ON_OFF,
        "sharenfs" | "sharesmb" => &[
            ("off", "not shared"),
            ("on", "shared with default options"),
            ("<options>", "shared with these options, e.g. rw=@10.0.0.0/24,no_root_squash"),
        ],
        _ => return None,
    })
}

pub fn pool_options(name: &str) -> Option<Opts> {
    if name.starts_with("feature@") {
        return Some(&[
            ("disabled", "not available; can be turned on (one-way)"),
            ("enabled", "available, not used yet: the pool still imports on older ZFS"),
            ("active", "in use on disk: only ZFS versions that know this feature can import the pool"),
        ]);
    }
    Some(match name {
        "failmode" => &[
            ("wait", "block IO until the failed devices return (default)"),
            ("continue", "return errors to new writes, keep serving reads"),
            ("panic", "crash the machine (for clusters that fail over)"),
        ],
        "autoexpand" | "autoreplace" | "autotrim" | "listsnapshots" | "delegation" | "multihost" | "readonly" => ON_OFF,
        _ => return None,
    })
}

fn size_of(label: &str) -> Option<u64> {
    let (n, unit) = label.split_at(label.find(|c: char| !c.is_ascii_digit()).unwrap_or(label.len()));
    let n: u64 = n.parse().ok()?;
    Some(match unit {
        "" => n,
        "K" => n << 10,
        "M" => n << 20,
        _ => return None,
    })
}

/// Does option `label` describe the current `value` (with numeric value `num`)?
pub fn option_matches(label: &str, value: &str, num: Option<u64>) -> bool {
    if let (Some(n), Some(l)) = (num, size_of(label)) {
        return n == l;
    }
    if let Some(prefix) = label.strip_suffix("-N") {
        return value
            .strip_prefix(prefix)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|r| r.chars().all(|c| c.is_ascii_digit()));
    }
    if label == "<options>" {
        return value != "on" && value != "off" && !value.is_empty() && value != "-";
    }
    label.eq_ignore_ascii_case(value)
}

/// (first OpenZFS release supporting it, what makes it become active) for pool features.
pub fn feature_info(name: &str) -> (&'static str, &'static str) {
    match name {
        "async_destroy" => ("0.6.1", "while a large dataset is being destroyed"),
        "empty_bpobj" => ("0.6.1", "when a snapshot is taken (on first use)"),
        "lz4_compress" => ("0.6.1", "as soon as it is enabled (always active once enabled)"),
        "multi_vdev_crash_dump" => ("0.6.4", "when a crash-dump volume is configured (illumos)"),
        "spacemap_histogram" => ("0.6.3", "immediately when enabled"),
        "enabled_txg" => ("0.6.3", "immediately when enabled"),
        "hole_birth" => ("0.6.3", "immediately when enabled"),
        "extensible_dataset" => ("0.6.3", "when a feature that needs it is used"),
        "embedded_data" => ("0.6.4", "immediately when enabled"),
        "bookmarks" => ("0.6.4", "when you create a bookmark (zfs bookmark)"),
        "filesystem_limits" => ("0.6.4", "when filesystem_limit or snapshot_limit is set"),
        "large_blocks" => ("0.6.5", "when a dataset uses recordsize > 128K"),
        "large_dnode" => ("0.7.0", "when a dataset uses dnodesize other than legacy"),
        "sha512" => ("0.7.0", "when a dataset uses checksum=sha512"),
        "skein" => ("0.7.0", "when a dataset uses checksum=skein"),
        "edonr" => ("2.0", "when a dataset uses checksum=edonr"),
        "userobj_accounting" => ("0.7.0", "immediately (per dataset, as it is next mounted)"),
        "encryption" => ("0.8.0", "when an encrypted dataset is created"),
        "project_quota" => ("0.8.0", "immediately (per dataset, as it is next mounted)"),
        "device_removal" => ("0.8.0", "when a top-level vdev is removed (zpool remove)"),
        "obsolete_counts" => ("0.8.0", "after a device removal"),
        "zpool_checkpoint" => ("0.8.0", "while a checkpoint exists (zpool checkpoint)"),
        "spacemap_v2" => ("0.8.0", "immediately when enabled"),
        "allocation_classes" => ("0.8.0", "when a special or dedup vdev is added"),
        "resilver_defer" => ("0.8.0", "when a resilver is deferred"),
        "bookmark_v2" => ("0.8.0", "when a bookmark of an encrypted/redacted dataset is created"),
        "redaction_bookmarks" | "redacted_datasets" => ("2.0", "when a redacted send/receive is done"),
        "bookmark_written" => ("2.0", "when a bookmark records written space"),
        "log_spacemap" => ("2.0", "immediately when enabled"),
        "livelist" => ("2.0", "when a clone is being deleted"),
        "device_rebuild" => ("2.0", "during a sequential rebuild (zpool replace -s, dRAID)"),
        "zstd_compress" => ("2.0", "when a dataset uses compression=zstd"),
        "draid" => ("2.1", "when a dRAID vdev is created"),
        "zilsaxattr" => ("2.2", "when an xattr=sa dataset logs a sync xattr change"),
        "head_errlog" => ("2.2", "immediately when enabled"),
        "blake3" => ("2.2", "when a dataset uses checksum=blake3"),
        "block_cloning" => ("2.2", "when a file is cloned (cp --reflink, copy_file_range)"),
        "vdev_zaps_v2" | "avz_v2" => ("2.2", "immediately when enabled"),
        "redaction_list_spill" => ("2.2", "when a redaction list grows large"),
        "raidz_expansion" => ("2.3", "when a disk is added to a raidz vdev (zpool attach)"),
        "fast_dedup" => ("2.3", "when a dedup table is created in the new format"),
        "longname" => ("2.3", "when a dataset with longname=on gets a long file name"),
        "large_microzap" => ("2.3", "when a directory grows a large microzap"),
        "block_cloning_endian" => ("2.3", "with block cloning (fixes cross-endian clone tables)"),
        "dynamic_gang_header" => ("2.4", "when a gang block uses the larger header"),
        "physical_rewrite" => ("2.4", "when zfs rewrite -P is used"),
        _ => ("?", "see man zpool-features"),
    }
}

/// Collapse runs of whitespace (help strings are wrapped in the source).
pub fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Help detail as display lines: paragraphs split on newlines, whitespace inside each collapsed.
pub fn paragraphs(s: &str) -> Vec<String> {
    s.split('\n').map(words).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage() {
        for p in ["used", "recordsize", "compression", "sync", "special_small_blocks", "com.sun:auto-snapshot"] {
            assert!(dataset_prop(p).is_some(), "{p}");
        }
        for p in ["capacity", "fragmentation", "feature@async_destroy", "feature@unknown_future_thing"] {
            assert!(pool_prop(p).is_some(), "{p}");
        }
        assert!(!feature("block_cloning").1.is_empty());
        for p in TXG_PARAMS.iter().chain(ARC_PARAMS.iter()) {
            assert!(param(p).is_some(), "{p}");
        }
    }
}

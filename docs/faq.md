# FAQ

[Türkçe](faq.tr.md)

## I just deleted something important. What now?

Stop using the drive it was on. Do not install anything there (including recovery software), do not download or save files to it, and close programs that write in the background. Then check, in this order: the Recycle Bin, Previous versions (shadow copies), cloud recycle bins (OneDrive, Google Drive, Dropbox), File History, and the app's own autosaves. Only then scan for deleted files. Unlost's first screen walks through the same order.

## Why must recovered files go to another disk?

Deleted files still sit in space the file system now calls free. Writing anything to that disk, including the recovered files themselves, can land on that space and destroy what you are trying to save. Unlost compares physical disk numbers, so two partitions of the same disk (for example `E:` and `F:` on one USB stick) are refused too.

## My drive is an SSD. Is there any hope?

Often less than on a hard disk. Windows sends TRIM for freed space on SSDs, and most SSDs then erase those blocks soon after. Unlost will usually still find the names, but the content reads as zeros; those files are marked "Zeroed". Small files stored inside the file record ("resident") can survive, and intact copies (Recycle Bin, shadow copies, cloud bins) are unaffected. USB sticks and SD cards usually do not receive TRIM.

## What do the states mean?

- **Intact copy:** a complete copy Windows kept (Recycle Bin, shadow copy).
- **Likely intact:** nothing else uses the file's space and its content is there. Not checked by a decoder, so open it to be sure.
- **Partial:** some of the space was reused, the start of the file does not look like its type, or parts could not be read. Saved as `PARTIAL_<name>`.
- **Zeroed:** the space is free but reads as zeros, typical after TRIM. Saved as `ZEROED_<name>` if you recover it anyway.
- **Overwritten:** other files now occupy nearly all of its space.
- **Name only:** the name and dates survive, but not where the content was.
- **Not recoverable here:** encrypted with EFS, an online-only cloud file, or compressed in a way this version cannot read.

Each file also shows the reason, with numbers.

## Why does Unlost ask for administrator access?

Reading a drive directly and opening shadow copies require it. Unlost does not run the whole app elevated: a separate helper gets the rights, reads, and sends data back to the window over a channel only your account can open. The helper never writes files. Recycle Bin recovery and scanning disk image files need no administrator access.

## Can it recover files from my phone?

Not when the phone is connected by USB. Phones connect as media devices (MTP): Windows shows their files but not their storage, so no PC tool can scan them for deleted files. Check the phone's Recently deleted album and your cloud photo trash (Google Photos keeps backed-up items for 60 days). A removable SD card that was not set up as internal storage can be scanned in a card reader.

## Why "Restore" is not offered for Recycle Bin items

Restoring to the original location writes to the same drive, which can overwrite other deleted files you may still want. Unlost copies items to a different disk and leaves the Recycle Bin as it is. You can still restore from Explorer afterwards.

## Does Unlost create a restore point or shadow copy before scanning?

No, never. Creating one writes to the volume being recovered.

## Does Unlost send anything over the internet?

No. It makes no network connections. The help links on the "Other places" page open in your browser only when you click them.

## Why does Unlost ask for a folder before scanning my Windows drive?

A busy Windows drive can hold millions of deleted file records. Unlost keeps the list in a small database while you browse it, and that database must not be written to the disk you are scanning. Its usual place is the app's data folder (`%LOCALAPPDATA%\Unlost`, or `data` next to the portable exe); when that folder is on the same physical disk as the drive you scan, Unlost asks for a folder on another disk, such as the one you will recover to, and checks it the same way as a recovery destination. It creates `Unlost scan data` there and deletes it when you close the scan or the app. A scan of a 1 TB Windows drive with 3 million deleted files used about 650 MB there.

## Where is the report?

In the recovery folder, as `unlost-report.csv` and `unlost-report.json`: original path, recovered path, size, method, state, reason, SHA-256 and whether the written file was read back and matched.

## Is it safe to run Unlost from the disk I am recovering from?

Running any program writes a little to its disk (Windows Prefetch, the app's own profile). For the best chance, run the portable build from a USB stick or another disk. The first screen warns when the app's own disk is the one you selected.

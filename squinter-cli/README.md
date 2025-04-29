# Squinter-CLI &emsp; [![Latest Version]][crates.io] [![Documentation]][docs.rs]

[Latest Version]: https://img.shields.io/crates/v/squinter-cli.svg
[crates.io]: https://crates.io/crates/squinter-cli

A set of command-line utilities that make use of the Squinter **Squ**ashFS **inter**face library.
Currently, two commands are included.

## sqcmd
This command allows you to perform read operations within a SquashFS image as if it were a mounted
filesystem. The command live-parses the SquashFS to provide output and behavior similar to well-
known UNIX shell commands.

General syntax:
```shell
sqcmd <FILESYSTEM> <COMMAND> [COMMAND ARGUMENTS]
```
where
* **FILESYSTEM**: The SquashFS image file or device to act on
* **COMMAND**: The command to run within the SquashFS (ls, cat)
* **COMMAND ARGUMENTS**: Command-specific arguments (see below)

Individual commands are described below.

### ls
List directory contents. Supports the '-l' flag for detailed listing. Does not support other
options supported by the real ls command.
```shell
$ sqcmd test.squashfs ls /bin
ash               dd       gzip        mount     ps      traceroute
board_detect      df       ipcalc.sh   mv        pwd     traceroute6
busybox           dmesg    kill        netmsg    rm      true
cat               echo     ln          netstat   rmdir   ubus
chgrp             egrep    lock        nice      sed     uclient-fetch
chmod             false    login       opkg      sh      umount
chown             fgrep    ls          passwd    sleep   uname
config_generate   fsync    mkdir       pidof     sync    vi
cp                grep     mknod       ping      tar     zcat
date              gunzip   mktemp      ping6     touch

$ sqcmd test.squashfs ls -l /bin
total 59
lrwxrwxrwx       -  ash -> busybox
-rwxr-xr-x     205  board_detect
-rwxr-xr-x  458773  busybox
lrwxrwxrwx       -  cat -> busybox
...output lines omitted...
lrwxrwxrwx       -  uname -> busybox
lrwxrwxrwx       -  vi -> busybox
lrwxrwxrwx       -  zcat -> busybox
```

### cat
Output file contents to stdout. Does not support any additional options beyond the files to output.
```shell
$ sqcmd test.squashfs cat /etc/passwd
root:x:0:0:root:/root:/bin/ash
daemon:*:1:1:daemon:/var:/bin/false
ftp:*:55:55:ftp:/home/ftp:/bin/false
network:*:101:101:network:/var:/bin/false
nobody:*:65534:65534:nobody:/var:/bin/false
ntp:x:123:123:ntp:/var/run/ntp:/bin/false
dnsmasq:x:453:453:dnsmasq:/var/run/dnsmasq:/bin/false
logd:x:514:514:logd:/var/run/logd:/bin/false
ubus:x:81:81:ubus:/var/run/ubus:/bin/false
```

## unsqfs
This is a mostly useless partial clone of unsquashfs. It currently does not support filesystem
extraction. It only supports listing (-l) and cat'ing (--cat) the filesystem contents.

General syntax:
```shell
unsqfs [OPTIONS] <FILESYSTEM> [FILES]...
```
where
* **FILESYSTEM**: The SquashFS image file or device to act on
* **Options**:
  * **-l**: List all files under the given paths (default: /)
  * **--cat**: Print the contents of all files under the given paths (default: /)

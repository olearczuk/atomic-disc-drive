# Overview
Goal of this project was to implement a distributed block device. It stores data in a distributed register, and can be used like any other block device.

Distributed register consists of multiple processes running in user space, possibly on many machines.
The block device driver connects to them using TCP. Also, the processes themselves communicate using TCP.
Processes can crash and recover at any time. Number of processes is fixed before the system is run, and every process has its own directory on the disc.
By process, I mean a single entity from a distributed register, not an operating system process.

Every sector of the block device is a separate atomic value stored in the distributed system, meaning the system supports a set of atomic values. called *registers*.
Sectors have 4096 bytes.

## Client commands
Let's begin with description of the interface of distributed system as seen by clients.
Every process of the system can be contacted by a client. Clients connect using TCP, and can send *READ* and *WRITE* commands.
System issues replies after a particular command was safely completed by the distributed system. Numbers are always in network byte order.

*READ* and *WRITE* have the following format:

    0       7 8     15 16    23 24    31 32    39 40    47 48    55 56    64
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |             Magic number          |       Padding            |   Msg  |
    | 0x61     0x74      0x64     0x64  |                          |   Type |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           Request number                              |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           Sector index                                |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           Command content ...
    |
    +--------+--------+--------...
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+

*READ* operation type is 0x01 and 0x02 stands for *WRITE*. Client can use request number for internal identification of messages.

*WRITE* has content with 4096 bytes with bytes to be written. *READ* has no content.
HMAC tag is `hmac(256)` tag of the entire message (from magic number to the end of content).

After the system completes any of those two operations, it must reply with response in the following format:

    0       7 8     15 16    23 24    31 32    39 40    47 48    55 56    64
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |             Magic number          |     Padding     | Status |  Msg   |
    | 0x61     0x74      0x64     0x64  |                 |  Code  |  Type  |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           Request number                              |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           Response content ...
    |
    +--------+--------+--------...
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+
    |                           HMAC tag                                    |
    |                                                                       |
    +--------+--------+--------+--------+--------+--------+--------+--------+

Again HMAC tag is `hmac(sha256)` of the entire response. Status codes will be explained shortly.
Response content for successful *WRITE* is empty, while for successful *READ* it has 4096
bytes read from the system. If a command has failed for any reason, it has empty
response content. Response message type is always 0x40 added to original message type,
so READ* response type is 0x41, while *WRITE* has 0x42.

Requests with invalid HMAC are discarded with appropriate status code returned.
A HMAC key for client commands and responses is in the *Configuration.hmac_client_key*
(`solution/src/domain.rs`) and is shared with client externally by tests.
This is not the cleanest cryptographic setup, but this scheme is easy to implement.

### Response status codes

Possible status codes are listed in *StatusCode* `solution/src/domain.rs`.
Doc strings in this file explain when each code is expected. Status codes are to be
represented as single consecutive bytes, starting with 0x00 as *Ok*.

# Distributed register

Here I describe what algorithm that is used by the register.

## (N, N)-AtomicRegister

There is a fixed number of processes, they know about each other. Crashes of individual
processes can happen. The `(N, N)-AtomicRegister` takes its name from the fact that
every process can initiate both read and write operation. For such a register, we assume
that at least majority of the processes are working correctly for it to be able to
make progress on operations. Here we provide the core algorithm, based on the
*Reliable and Secure Distributed Programming* by C. Cachin, R. Guerraoui, L. Rodrigues and
modified to suit crash-recovery model:

```text
Implements:
    (N,N)-AtomicRegister instance nnar.

Uses:
    StubbornBestEffortBroadcast, instance sbeb;
    StubbornLinks, instance sl;

upon event < nnar, Init > do
    (ts, wr, val) := (0, 0, _);
    rid:= 0;
    readlist := [ _ ] `of length` N;
    acklist := [ _ ] `of length` N;
    reading := FALSE;
    writing := FALSE;
    writeval := _;
    readval := _;
    store(wr, ts, val, rid, writing, writeval);

upon event < nnar, Recovery > do
    retrieve(wr, ts, val, rid, writing, writeval);
    readlist := [ _ ] `of length` N;
    acklist := [ _ ]  `of length` N;
    reading := FALSE;
    readval := _;
    if writing = TRUE then
        trigger < sbeb, Broadcast | [READ_PROC, rid] >;

upon event < nnar, Read > do
    rid := rid + 1;
    store(rid);
    readlist := [ _ ] `of length` N;
    acklist := [ _ ] `of length` N;
    reading := TRUE;
    trigger < sbeb, Broadcast | [READ_PROC, rid] >;

upon event < sbeb, Deliver | p [READ_PROC, r] > do
    trigger < pl, Send | p, [VALUE, r, ts, wr, val] >;

upon event <sl, Deliver | q, [VALUE, r, ts', wr', v'] > such that r == rid do
    readlist[q] := (ts', wr', v');
    if #(readlist) > N / 2 and (reading or writing) then
        (maxts, rr, readval) := highest(readlist);
        readlist := [ _ ] `of length` N;
        acklist := [ _ ] `of length` N;
        rod := rid + 1 // Added
        if reading = TRUE then
            trigger < sbeb, Broadcast | [WRITE_PROC, rid, maxts, rr, readval] >;
        else
            trigger < sbeb, Broadcast | [WRITE_PROC, rid, maxts + 1, rank(self), writeval] >;

upon event < nnar, Write | v > do
    rid := rid + 1;
    writeval := v;
    acklist := [ _ ] `of length` N;
    readlist := [ _ ] `of length` N;
    writing := TRUE;
    store(rid, writeval, writing);
    trigger < sbeb, Broadcast | [READ_PROC, rid] >;

upon event < sbeb, Deliver | p, [WRITE_PROC, r, ts', wr', v'] > do
    if (ts', wr') > (ts, wr) then
        (ts, wr, val) := (ts', wr', v');
        store(ts, wr, val);
    trigger < pl, Send | p, [ACK, r] >;

upon event < pl, Deliver | q, [ACK, r] > such that r == rid do
    acklist[q] := Ack;
    if #(acklist) > N / 2 and (reading or writing) then
        acklist := [ _ ] `of length` N;
        if reading = TRUE then
            reading := FALSE;
            trigger < nnar, ReadReturn | readval >;
        else
            writing := FALSE;
            store(writing);
            trigger < nnar, WriteReturn >;
```

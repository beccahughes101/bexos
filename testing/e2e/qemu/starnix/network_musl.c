typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ssize_t;
typedef unsigned short u16;
typedef unsigned int u32;

extern int accept4(int, void *, void *, int);
extern int bind(int, const void *, u32);
extern int connect(int, const void *, u32);
extern int listen(int, int);
extern int open(const char *, int, ...);
extern int puts(const char *);
extern ssize_t read(int, void *, size_t);
extern ssize_t recv(int, void *, size_t, int);
extern ssize_t recvfrom(int, void *, size_t, int, void *, void *);
extern ssize_t send(int, const void *, size_t, int);
extern ssize_t sendto(int, const void *, size_t, int, const void *, u32);
extern int setns(int, int);
extern int setsockopt(int, int, int, const void *, u32);
extern int socket(int, int, int);
extern int unshare(int);
extern ssize_t write(int, const void *, size_t);

struct sockaddr_in { u16 family, port; u32 address; unsigned char zero[8]; };
struct sockaddr_in6 { u16 family, port; u32 flow; unsigned char address[16]; u32 scope; };
struct nlmsghdr { u32 length; u16 type, flags; u32 sequence, pid; };
struct rtgenmsg { unsigned char family; };
struct link_request { struct nlmsghdr header; struct rtgenmsg body; };
struct diag_request { struct nlmsghdr header; unsigned char family, protocol, ext, pad; u32 states; };
struct nf_request { struct nlmsghdr header; unsigned char family, version; u16 resource; };

static u16 network16(u16 value) { return (u16)((value << 8) | (value >> 8)); }
static int fail(const char *what) { puts(what); return 1; }
static u16 checksum(const unsigned char *bytes, size_t length) {
  u32 sum = 0;
  while (length >= 2) {
    sum += ((u16)bytes[0] << 8) | bytes[1];
    bytes += 2; length -= 2;
  }
  if (length) sum += (u16)bytes[0] << 8;
  while (sum >> 16) sum = (sum & 0xffff) + (sum >> 16);
  return (u16)~sum;
}
static int bind_backend(const char *name, u32 destination) {
  int fd = socket(2, 2 | 0x80000, 0);
  struct sockaddr_in address = {2, network16(9), destination, {0}};
  size_t length = 0;
  while (name[length]) ++length;
  return fd < 0 || setsockopt(fd, 1, 25, name, (u32)length + 1) < 0 ||
         connect(fd, &address, sizeof(address)) < 0 ? -1 : fd;
}

int main(void) {
  enum { AF_INET = 2, AF_INET6 = 10, AF_NETLINK = 16, AF_PACKET = 17 };
  enum { SOCK_STREAM = 1, SOCK_DGRAM = 2, SOCK_RAW = 3, SOCK_CLOEXEC = 0x80000 };
  int server = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int client = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (server < 0 || client < 0) return fail("network fixture: UDP socket");
  struct sockaddr_in address = {AF_INET, network16(24570), 0x0100007f, {0}};
  if (bind(server, &address, sizeof(address)) < 0) return fail("network fixture: UDP bind");
  if (sendto(client, "udp", 3, 0, &address, sizeof(address)) != 3)
    return fail("network fixture: UDP send");
  char buffer[256] = {0};
  if (recv(server, buffer, sizeof(buffer), 0) != 3 || buffer[0] != 'u')
    return fail("network fixture: UDP receive");

  int listener = socket(AF_INET6, SOCK_STREAM | SOCK_CLOEXEC, 0);
  int connector = socket(AF_INET6, SOCK_STREAM | SOCK_CLOEXEC, 0);
  struct sockaddr_in6 address6 = {AF_INET6, network16(24571), 0, {0}, 0};
  address6.address[15] = 1;
  if (listener < 0 || connector < 0 || bind(listener, &address6, sizeof(address6)) < 0 ||
      listen(listener, 4) < 0 || connect(connector, &address6, sizeof(address6)) < 0)
    return fail("network fixture: TCP setup");
  int accepted = accept4(listener, 0, 0, SOCK_CLOEXEC);
  if (accepted < 0 || write(connector, "tcp", 3) != 3 ||
      read(accepted, buffer, sizeof(buffer)) != 3 || buffer[0] != 't')
    return fail("network fixture: TCP transfer");

  int route = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, 0);
  struct link_request request = {{sizeof(request), 18, 0x301, 1, 0}, {0}};
  if (route < 0 || send(route, &request, sizeof(request), 0) < 0 ||
      recv(route, buffer, sizeof(buffer), 0) < 0)
    return fail("network fixture: route netlink");
  int diag = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, 4);
  struct diag_request diag_query = {{sizeof(diag_query), 20, 0x301, 2, 0}, 2, 6, 0, 0, ~0u};
  if (diag < 0 || send(diag, &diag_query, sizeof(diag_query), 0) < 0 ||
      recv(diag, buffer, sizeof(buffer), 0) < 0)
    return fail("network fixture: sock diag");
  int netfilter = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, 12);
  struct nf_request nf_query = {{sizeof(nf_query), 0x0a10, 0x301, 3, 0}, 2, 0, 0};
  if (netfilter < 0 || send(netfilter, &nf_query, sizeof(nf_query), 0) < 0 ||
      recv(netfilter, buffer, sizeof(buffer), 0) < 0)
    return fail("network fixture: netfilter netlink");
  if (socket(AF_PACKET, SOCK_RAW | SOCK_CLOEXEC, network16(0x0800)) < 0)
    return fail("network fixture: AF_PACKET");
  if (bind_backend("direct0", 0x01010101) < 0 ||
      bind_backend("eth0", 0x0202000a) < 0 ||
      bind_backend("eth1", 0x0100010a) < 0)
    return fail("network fixture: multi-vNIC binding");

  unsigned char echo4[8] = {8, 0, 0, 0, 0x12, 0x34, 0, 1};
  u16 sum = checksum(echo4, sizeof(echo4));
  echo4[2] = (unsigned char)(sum >> 8); echo4[3] = (unsigned char)sum;
  int icmp4 = socket(AF_INET, SOCK_RAW | SOCK_CLOEXEC, 1);
  if (icmp4 < 0 || sendto(icmp4, echo4, sizeof(echo4), 0, &address, sizeof(address)) != 8 ||
      recvfrom(icmp4, buffer, sizeof(buffer), 0, 0, 0) < 8 || (unsigned char)buffer[0] != 0)
    return fail("network fixture: IPv4 ICMP");

  unsigned char echo6[8] = {128, 0, 0, 0, 0x12, 0x34, 0, 2};
  int icmp6 = socket(AF_INET6, SOCK_RAW | SOCK_CLOEXEC, 58);
  if (icmp6 < 0 || sendto(icmp6, echo6, sizeof(echo6), 0, &address6, sizeof(address6)) != 8 ||
      recvfrom(icmp6, buffer, sizeof(buffer), 0, 0, 0) < 8 || (unsigned char)buffer[0] != 129)
    return fail("network fixture: IPv6 ICMP");

  int original = open("/proc/self/ns/net", 0x80000);
  if (original < 0 || unshare(0x40000000) < 0 ||
      socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0) < 0 ||
      setns(original, 0x40000000) < 0)
    return fail("network fixture: namespace round trip");
  puts("starnix networking ok");
  return 0;
}

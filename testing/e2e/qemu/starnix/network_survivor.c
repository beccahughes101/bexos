typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ssize_t;
typedef unsigned short u16;
typedef unsigned int u32;

extern int connect(int, const void *, u32);
extern int open(const char *, int, ...);
extern int puts(const char *);
extern ssize_t read(int, void *, size_t);
extern ssize_t recv(int, void *, size_t, int);
extern unsigned int sleep(unsigned int);
extern ssize_t send(int, const void *, size_t, int);
extern int setsockopt(int, int, int, const void *, u32);
extern int setns(int, int);
extern int socket(int, int, int);
extern int unshare(int);
extern ssize_t write(int, const void *, size_t);

struct sockaddr_in {
  u16 family;
  u16 port;
  u32 address;
  unsigned char zero[8];
};

static u16 network16(u16 value) { return (u16)((value << 8) | (value >> 8)); }

static int connected(int kind, const char *interface_name) {
  int fd = socket(2, kind | 0x80000, 0);
  size_t length = 0;
  while (interface_name[length]) ++length;
  struct sockaddr_in peer = {2, network16(34567), 0x0202000a, {0}};
  if (fd < 0 || setsockopt(fd, 1, 25, interface_name, (u32)length + 1) < 0 ||
      connect(fd, &peer, sizeof(peer)) < 0)
    return -1;
  return fd;
}

static int exchange_tcp(int fd, char value) {
  char reply = 0;
  return write(fd, &value, 1) == 1 && read(fd, &reply, 1) == 1 &&
         reply == value;
}

static int exchange_udp(int fd, char value) {
  char reply = 0;
  return send(fd, &value, 1, 0) == 1 && recv(fd, &reply, 1, 0) == 1 &&
         reply == value;
}

int main(void) {
  int direct_tcp = connected(1, "direct0");
  int l2_tcp = connected(1, "eth0");
  int direct_udp = connected(2, "direct0");
  int l2_udp = connected(2, "eth0");
  if (direct_tcp < 0 || l2_tcp < 0 || direct_udp < 0 || l2_udp < 0) {
    puts("starnix network survivor: connect failed");
    return 1;
  }
  int original_namespace = open("/proc/self/ns/net", 0x80000);
  if (original_namespace < 0 || unshare(0x40000000) < 0 ||
      socket(2, 2 | 0x80000, 0) < 0 ||
      setns(original_namespace, 0x40000000) < 0) {
    puts("starnix network survivor: namespace failed");
    return 1;
  }
  unsigned long long sequence = 0;
  for (;;) {
    char value = (char)('a' + sequence % 26);
    if (!exchange_tcp(direct_tcp, value) || !exchange_tcp(l2_tcp, value) ||
        !exchange_udp(direct_udp, value) || !exchange_udp(l2_udp, value)) {
      puts("starnix network survivor: flow failed");
      return 2;
    }
    char line[] = "starnix net flow 0000000000000000";
    static const char hex[] = "0123456789abcdef";
    unsigned long long current = sequence++;
    for (int index = 0; index < 16; ++index) {
      line[32 - index] = hex[current & 15];
      current >>= 4;
    }
    puts(line);
    sleep(1);
  }
}

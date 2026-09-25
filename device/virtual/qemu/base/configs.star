load("//device/base:configs.star", "base_system")

def _value(name, value):
    return {"name": name, "value": value}

def _component(package_id, values):
    return {"package_id": package_id, "values": values}

def qemu_system(product, architecture, graphics = False, development = False, workstation_services = True):
    component_config_values = [
        _component("bexos.platform.storage_verify", [
            _value("retry_limit", {"uint32_value": 3}),
            _value("channel", {"string_value": "qemu"}),
        ]),
    ]
    if workstation_services:
        component_config_values.extend([
            _component("bexos.service.netstackd", [
                _value("static_ipv4", {"bytes_value": b"\x0a\x00\x02\x0f"}),
                _value("static_prefix_len", {"uint32_value": 24}),
                _value("static_gateway", {"bytes_value": b"\x0a\x00\x02\x02"}),
                _value("static_dns", {"bytes_value": b"\x0a\x00\x02\x03"}),
                _value("static_ipv6", {"bytes_value": b"\xfe\xc0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x15"}),
                _value("static_ipv6_prefix_len", {"uint32_value": 64}),
                _value("static_ipv6_gateway", {"bytes_value": b"\xfe\xc0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02"}),
                _value("static_dns_ipv6", {"bytes_value": b"\xfe\xc0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03"}),
                _value("doh_bootstrap_ipv6", {"bytes_value": b"\xfe\xc0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03"}),
                _value("slaac_enabled", {"bool_value": True}),
                _value("dhcp_enabled", {"bool_value": True}),
                _value("dhcp_timeout_ms", {"uint32_value": 3000}),
                _value("dns_mode", {"string_value": "udp53"}),
                _value("doh_host", {"string_value": "dns.google"}),
                _value("doh_path", {"string_value": "/dns-query"}),
                _value("doh_port", {"uint32_value": 443}),
                _value("doh_strict", {"bool_value": False}),
                _value("dns_cache_capacity", {"uint32_value": 64}),
            ]),
            _component("bexos.service.networkd", [
                _value("instance_id", {"string_value": "system_default"}),
                _value("domains", {"string_value": "system_default:0"}),
                _value("max_dynamic_providers", {"uint32_value": 64}),
                _value("dns_cache_capacity", {"uint32_value": 256}),
            ]),
            _component("bexos.service.timed", [
                _value("primary_server", {"string_value": "time.google.com"}),
                _value("poll_interval_ms", {"uint32_value": 60000}),
                _value("initial_retry_ms", {"uint32_value": 1000}),
                _value("use_nts", {"bool_value": False}),
                _value("slew_limit_ppm", {"uint32_value": 500}),
                _value("slew_step_threshold_ns", {"uint64_value": 1000000000}),
            ]),
        ])
    return base_system(
        product_name = product + "_" + architecture + ("_development" if development else ""),
        board = "//device/virtual/qemu/base/" + architecture,
        platform_config = "//device/virtual/qemu/" + product + (":platform_config_emulated_bin" if development else ":platform_config_bin"),
        bundles = ["base", "qemu_hardware"] + (["graphics", "qemu_graphics"] if graphics else []),
        component_config_values = component_config_values,
    )

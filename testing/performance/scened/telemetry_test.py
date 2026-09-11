import unittest
from telemetry import collect


class TelemetryTests(unittest.TestCase):
    def test_units_and_duplicate_serial_sections_do_not_create_samples(self):
        observations = '\n'.join([
            'scened-metrics: {"main_wall_p99_us": 42, "gpu_queue_interval_p99_ns": null, "hardware_vsync": false}',
            'input-fixture: worker timing {"worker_us": 3000000, "gpu_queue_interval_ns": 2900000000, "hardware_performance_verified": false}',
            'input hotplug host-observed latency ms: {"host_inject_to_client_log": 101.5}',
            'service-transplant: committed generation=92 cutover_ms=82',
        ])
        report = collect(observations + '\n' + observations)
        self.assertEqual(len(report['compositor_windows']), 1)
        self.assertEqual(report['gpu_worker_observations'][0]['gpu_queue_interval_ns'], 2900000000)
        self.assertEqual(report['input_latency_observations_ms'][0]['host_inject_to_client_log'], 101.5)
        self.assertEqual(report['cutovers'], [{'generation': 92, 'wall_ms': 82}])
        self.assertFalse(report['integration_passed'])
        self.assertFalse(report['guest_cpu_accounting_available'])
        self.assertFalse(report['guest_allocation_counts_available'])

    def test_functional_success_does_not_assert_hardware_acceptance(self):
        report = collect('scened: Vello/Venus composed frame submitted\nBEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED\nVENUS_LINUX_FIXTURE_EXIT=0')
        self.assertTrue(report['integration_passed'])
        self.assertTrue(report['compositor_gpu_frame_observed'])
        self.assertFalse(report['hardware_performance_verified'])

    def test_failure_diagnostics_cannot_supply_success_markers(self):
        report = collect('AssertionError: missing fixture event: scened: Vello/Venus composed frame submitted\nassert "BEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED" in output\nVENUS_LINUX_FIXTURE_EXIT=1')
        self.assertFalse(report['integration_passed'])
        self.assertFalse(report['compositor_gpu_frame_observed'])
        self.assertFalse(collect('BEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED')['integration_passed'])
        self.assertFalse(collect('SCENED_SUSTAINED_VERIFIED\nSCENED_NATIVE_FIXTURE_VERIFIED')['sustained_passed'])

    def test_corrupt_measurements_are_rejected(self):
        for packet in ['[]', '{}', '{"x": -1}', '{"x": NaN}', '{"x": Infinity}', '{"x": true}', '{"x": "42"}', '{"x": 18446744073709551616}', '{"x": 1} trailing']:
            with self.subTest(packet=packet), self.assertRaises(ValueError):
                collect('scened-metrics: ' + packet)


    def test_identified_windows_report_coverage_without_claiming_hardware(self):
        import json
        p = {"schema":3,"epoch_us":1,"window_id":1,"workload":2,"samples":256,"window":256,"cpu_samples":256,"process_cpu_p99_ns":7000,"allocations":10,"allocation_bytes":64,"allocation_failures":0}
        text = "scened-metrics: " + json.dumps(p)
        r = collect(text + "\n" + text + "\nSCENED_SUSTAINED_VERIFIED\nSCENED_NATIVE_FIXTURE_VERIFIED")
        self.assertTrue(r["guest_cpu_accounting_available"])
        self.assertTrue(r["guest_allocation_counts_available"])
        self.assertTrue(r["sustained_passed"])
        self.assertTrue(r["integration_passed"])
        self.assertEqual(r["measured_workloads"], [2])
        for field, value in [("epoch_us", 0), ("window_id", 0), ("workload", 6), ("cpu_samples", 257), ("worker_cpu_samples", 256.0), ("gpu_timestamp_samples", 257)]:
            invalid = dict(p, **{field: value})
            with self.subTest(field=field), self.assertRaises(ValueError):
                collect("scened-metrics: " + json.dumps(invalid))
        unavailable = dict(p, cpu_samples=0, process_cpu_p99_ns=None)
        r = collect("scened-metrics: " + json.dumps(unavailable))
        self.assertFalse(r["guest_cpu_accounting_available"])
        self.assertIsNone(r["compositor_windows"][0]["process_cpu_p99_ns"])
        p["samples"] = 255
        with self.assertRaises(ValueError): collect("scened-metrics: " + json.dumps(p))
        p["samples"] = 256; p["allocation_bytes"] = 65
        with self.assertRaises(ValueError): collect(text + "\nscened-metrics: " + json.dumps(p))

class WorkloadTests(unittest.TestCase):
    def test_focus_waits_for_fresh_down_before_starting_workload(self):
        from guest import Workloads
        requests = []
        w = Workloads(lambda name, args: requests.append((name, args)), [2], False)
        old = 'input-fixture: view=0 kind=1 phase=1\n'
        self.assertFalse(w.poll(old, None))
        self.assertEqual(len(requests), 2)
        self.assertTrue(requests[0][1]['events'][-1]['data']['down'])
        self.assertFalse(requests[1][1]['events'][0]['data']['down'])
        self.assertFalse(w.poll(old, None))
        self.assertEqual(len(requests), 2)
        self.assertFalse(w.poll(old + old, None))
        self.assertEqual(requests[-1][1]['events'][0]['data']['key']['data'], 'f2')
        self.assertTrue(w.started)

    def test_cpu_fallback_cannot_pass_gpu_workload(self):
        from guest import Workloads
        import json
        w=Workloads(lambda *args:None,[1],True)
        w.started=True; w.deadline=float("inf")
        packet={"workload":1,"window_id":1,"samples":256,"cpu_samples":256,"allocation_failures":0,"failures":0,"gpu_frames":0,"direct_frames":0,"worker_cpu_samples":0}
        log='scened-workload: complete workload=1 frames=288 leases=0\nscened-metrics: '+json.dumps(packet)
        with self.assertRaises(AssertionError): w.poll(log,lambda x,y:(167,83,41))

    def test_scanout_requires_actual_completions_and_retirement(self):
        from guest import Workloads
        import json
        w=Workloads(lambda *args:None,[5],False)
        w.started=True; w.deadline=float("inf")
        self.assertFalse(w.poll('scened-workload: complete workload=5 frames=288 leases=1',lambda x,y:(167,83,41)))
        packet={"workload":5,"window_id":1,"samples":256,"cpu_samples":256,"allocation_failures":0,"failures":0,"gpu_frames":0,"direct_frames":256}
        log='scened-workload: complete workload=5 frames=288 leases=0\nscened-metrics: '+json.dumps(packet)
        self.assertTrue(w.poll(bytearray(log.encode()),lambda x,y:(167,83,41)))

unittest.main()

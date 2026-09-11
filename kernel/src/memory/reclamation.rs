//! Small retirement work batches shared by syscall and scheduler maintenance.
pub(crate) fn maintain(rt: &mut crate::syscall::Rt) {
    let pending = rt.has_pending_reclamation();
    rt.reclaim_retired_pages(4);
    if pending && !rt.has_pending_reclamation() {
        crate::log_line("kernel: retired private memory fully reclaimed");
    }
}

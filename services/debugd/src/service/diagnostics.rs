use super::*;

pub(super) async fn exec_command<
    A: AppManager,
    U: UpdateManager,
    P: PlatformUpdateApplier,
    T: TeeManager,
>(
    component_id: &str,
    args: &[String],
    processes: &[ProcessInfo],
    apps: &mut A,
    updates: &mut U,
    platform: &mut P,
    tee: &mut T,
) -> ExecResponse {
    match component_id {
        "debugd.version" => ExecResponse {
            exit_code: 0,
            stdout: alloc::format!("{VERSION}\n"),
            stderr: String::new(),
        },
        "kernel.ps" => {
            let mut stdout = String::new();
            for process in processes {
                stdout.push_str(&alloc::format!(
                    "{}\t{}\t{}\t{}\n",
                    process.pid,
                    process.state,
                    process.name,
                    process.package_id
                ));
            }
            ExecResponse {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
        "shell.terminate_selected" => {
            let package = args.first().map_or("", String::as_str);
            let uid = args.get(1).and_then(|value| value.parse::<u64>().ok());
            if package.is_empty() || uid.is_none() || args.len() != 2 {
                status_exec(DebugStatusResponse {
                    status: -8,
                    message: "expected package and UID".into(),
                })
            } else {
                status_exec(apps.terminate_selected_shell(package, uid.unwrap()).await)
            }
        }
        "app.progress" => status_exec(
            apps.process_progress(
                args.first()
                    .map_or("bexos.platform.storage_verify", |s| s.as_str()),
            )
            .await,
        ),
        "update.status" => status_exec(updates.status().await),
        "update.apply_service" => status_exec(updates.apply_service(apps).await),
        "update.apply_stored_service" => {
            let target = args.first().map_or("", |s| s.as_str());
            let generation = args.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let archive_id = args.get(2).map_or(target, |s| s.as_str());
            status_exec(
                apps.migrate_service_from_store(archive_id, generation, target)
                    .await,
            )
        }
        "update.service.status" => status_exec(
            apps.migration_status(args.first().map_or("bexos.platform.appd", |s| s.as_str()))
                .await,
        ),
        "update.apply_app" => status_exec(updates.apply_app(apps).await),
        "update.apply_platform" => status_exec(updates.apply_platform(platform, tee).await),
        "update.apply_firmware" => {
            if args.len() != 1 || !matches!(args[0].as_str(), "live" | "on-reboot") {
                return status_exec(DebugStatusResponse {
                    status: -8,
                    message: "expected live or on-reboot activation".into(),
                });
            }
            status_exec(updates.apply_firmware(args[0] == "on-reboot").await)
        }
        "update.apply_firmware_from_feed" => {
            if args.len() != 2
                || !matches!(args[0].as_str(), "tee" | "hypervisor")
                || !matches!(args[1].as_str(), "live" | "on-reboot")
            {
                return status_exec(DebugStatusResponse {
                    status: -8,
                    message: "expected tee or hypervisor and live or on-reboot activation".into(),
                });
            }
            status_exec(
                updates
                    .apply_firmware_from_feed(
                        if args[0] == "tee" { 4 } else { 5 },
                        args[1] == "on-reboot",
                    )
                    .await,
            )
        }
        "kernel.update.status" => status_exec(platform.platform_update_status().await),
        "tee.info" => {
            let info = tee.info().await;
            status_exec(DebugStatusResponse {
                status: info.status,
                message: alloc::format!(
                    "{} kind={} secure_os_version={} anti_rollback_version={}",
                    if info.present { "present" } else { "absent" },
                    info.kind,
                    info.secure_os_version,
                    info.anti_rollback_version
                ),
            })
        }
        "tee.apps" => {
            let list = tee.list_apps().await;
            if list.status != 0 {
                return status_exec(DebugStatusResponse {
                    status: list.status,
                    message: "tee app list failed".into(),
                });
            }
            let mut stdout = String::new();
            for app in list.apps {
                stdout.push_str(&alloc::format!(
                    "{}\tversion={}\tsessions={}\t{}\n",
                    hex_uuid(&app.uuid),
                    app.version,
                    app.active_sessions,
                    app.entry_point_name
                ));
            }
            ExecResponse {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
        "tee.keymint_smoke" => status_exec(keymint_smoke(tee).await),
        "tee.orchestrator_smoke" => status_exec(orchestrator_smoke(tee).await),
        "tee.authmgr_smoke" => status_exec(authmgr_smoke(tee).await),
        "tee.concurrent_smoke" => status_exec(tee.concurrent_storage_probe().await),
        "tee.update.status" => {
            let status = tee.update_status().await;
            status_exec(DebugStatusResponse {
                status: status.status,
                message: alloc::format!(
                    "{} generation={} {}",
                    status.update_status,
                    status.generation,
                    status.message
                ),
            })
        }
        _ => ExecResponse {
            exit_code: 127,
            stdout: String::new(),
            stderr: "unsupported debug command\n".into(),
        },
    }
}

pub(super) async fn authmgr_smoke<T: TeeManager>(tee: &mut T) -> DebugStatusResponse {
    let open = tee
        .open_session(TeeUuidRequest {
            uuid: AUTHMGR_BE_UUID.to_vec(),
        })
        .await;
    if open.status != 0 {
        return DebugStatusResponse {
            status: open.status,
            message: "AuthMgr BE connection rejected".into(),
        };
    }
    // This normal-world probe checks service availability only. Secure
    // authorization acceptance/rejection is exercised by the isolated TA
    // through Binder, which exposes explicit AuthMgr error codes.
    let closed = tee
        .close_session(TeeSessionRequest {
            session_id: open.session_id,
        })
        .await;
    DebugStatusResponse {
        status: closed.status,
        message: if closed.status == 0 {
            "AuthMgr BE session opened and closed".into()
        } else {
            "AuthMgr BE session close failed".into()
        },
    }
}

pub(super) async fn orchestrator_smoke<T: TeeManager>(tee: &mut T) -> DebugStatusResponse {
    let open = tee
        .open_session(TeeUuidRequest {
            uuid: ORCHESTRATOR_UUID.to_vec(),
        })
        .await;
    if open.status != 0 {
        return DebugStatusResponse {
            status: open.status,
            message: "orchestrator session open failed".into(),
        };
    }
    let payload = match encode_orchestrator_request(ORCHESTRATOR_CMD_GET_KERNEL_SLOT, 1) {
        Ok(payload) => payload.to_vec(),
        Err(_) => return invalid_request_response("orchestrator request encode failed"),
    };
    let invoked = tee
        .invoke(TeeInvokeRequest {
            session_id: open.session_id,
            command_id: ORCHESTRATOR_CMD_GET_KERNEL_SLOT,
            payload,
        })
        .await;
    let _ = tee
        .close_session(TeeSessionRequest {
            session_id: open.session_id,
        })
        .await;
    if invoked.status != 0 {
        return DebugStatusResponse {
            status: invoked.status,
            message: "orchestrator invoke failed".into(),
        };
    }
    match decode_orchestrator_response(&invoked.response) {
        Ok(1) => DebugStatusResponse {
            status: 0,
            message: "orchestrator reachable; active slot A".into(),
        },
        Ok(slot) => DebugStatusResponse {
            status: -8,
            message: alloc::format!("unexpected orchestrator slot {slot}"),
        },
        Err(_) => DebugStatusResponse {
            status: -8,
            message: "invalid orchestrator response".into(),
        },
    }
}

pub(super) async fn keymint_smoke<T: TeeManager>(tee: &mut T) -> DebugStatusResponse {
    let open = tee
        .open_session(TeeUuidRequest {
            uuid: KEYMINT_UUID.to_vec(),
        })
        .await;
    if open.status != 0 {
        return DebugStatusResponse {
            status: open.status,
            message: "KeyMint session open failed".into(),
        };
    }

    let alias = "debugd:keymint-ed25519";
    let generate_payload =
        match encode_keymint_generate_key(1, alias, KeyMintAlgorithm::Ed25519, false, 0, None) {
            Ok(payload) => payload,
            Err(_) => {
                let _ = tee
                    .close_session(TeeSessionRequest {
                        session_id: open.session_id,
                    })
                    .await;
                return invalid_request_response("KeyMint generate request encode failed");
            }
        };
    let generated = tee
        .invoke(TeeInvokeRequest {
            session_id: open.session_id,
            command_id: KEYMINT_CMD_GENERATE_KEY,
            payload: generate_payload,
        })
        .await;
    if generated.status != 0 {
        let _ = tee
            .close_session(TeeSessionRequest {
                session_id: open.session_id,
            })
            .await;
        return DebugStatusResponse {
            status: generated.status,
            message: "KeyMint generate failed".into(),
        };
    }
    let decoded = decode_keymint_generated_key(&generated.response);
    let generated_key = match decoded {
        Ok(key) if !key.public_material.is_empty() => key,
        result => {
            let message = match result {
                Err(error) => alloc::format!("KeyMint Ed25519 generate response: {error:?}"),
                Ok(_) => "KeyMint Ed25519 certificate missing".into(),
            };
            let _ = tee
                .close_session(TeeSessionRequest {
                    session_id: open.session_id,
                })
                .await;
            return DebugStatusResponse {
                status: -8,
                message,
            };
        }
    };

    let signed = match keymint_operation(
        tee,
        open.session_id,
        KeyMintAlgorithm::Ed25519,
        KeyMintPurpose::Sign,
        &generated_key.opaque_blob,
        None,
        &[0x4b; 32],
        &[],
    )
    .await
    {
        Ok((_, output)) => output,
        Err(response) => {
            let _ = tee
                .close_session(TeeSessionRequest {
                    session_id: open.session_id,
                })
                .await;
            return response;
        }
    };
    if signed.len() != 64 {
        let _ = tee
            .close_session(TeeSessionRequest {
                session_id: open.session_id,
            })
            .await;
        return DebugStatusResponse {
            status: -8,
            message: "KeyMint sign returned invalid signature".into(),
        };
    }

    if let Err(response) = keymint_p256_smoke(tee, open.session_id).await {
        return response;
    }
    if let Err(response) = keymint_aes_gcm_smoke(tee, open.session_id).await {
        return response;
    }
    if let Err(response) = keymint_hmac_smoke(tee, open.session_id).await {
        return response;
    }
    if let Err(response) = keymint_delete_and_reject_smoke(
        tee,
        open.session_id,
        KeyMintAlgorithm::Ed25519,
        &generated_key.opaque_blob,
    )
    .await
    {
        return response;
    }
    let close = tee
        .close_session(TeeSessionRequest {
            session_id: open.session_id,
        })
        .await;
    if close.status != 0 {
        return DebugStatusResponse {
            status: close.status,
            message: "KeyMint close failed".into(),
        };
    }
    DebugStatusResponse {
        status: 0,
        message: "KeyMint hardware smoke passed".into(),
    }
}

pub(super) async fn keymint_p256_smoke<T: TeeManager>(
    tee: &mut T,
    session_id: u64,
) -> Result<(), DebugStatusResponse> {
    let alias = "debugd:keymint-p256";
    let generate_payload =
        encode_keymint_generate_key(1, alias, KeyMintAlgorithm::P256, false, 0, None)
            .map_err(|_| invalid_request_response("KeyMint P-256 generate encode failed"))?;
    let generated = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_GENERATE_KEY,
            payload: generate_payload,
        })
        .await;
    if generated.status != 0 {
        return Err(DebugStatusResponse {
            status: generated.status,
            message: "KeyMint P-256 generate failed".into(),
        });
    }
    let generated_key =
        decode_keymint_generated_key(&generated.response).map_err(|_| DebugStatusResponse {
            status: -8,
            message: "KeyMint P-256 returned invalid public key".into(),
        })?;
    if generated_key.public_material.is_empty() {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint P-256 returned invalid public key".into(),
        });
    }
    let (_, signature) = keymint_operation(
        tee,
        session_id,
        KeyMintAlgorithm::P256,
        KeyMintPurpose::Sign,
        &generated_key.opaque_blob,
        None,
        &[0x2b; 32],
        &[],
    )
    .await?;
    if !(64..=72).contains(&signature.len()) {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint P-256 returned invalid signature".into(),
        });
    }
    keymint_delete_and_reject_smoke(
        tee,
        session_id,
        KeyMintAlgorithm::P256,
        &generated_key.opaque_blob,
    )
    .await?;
    Ok(())
}

pub(super) async fn keymint_aes_gcm_smoke<T: TeeManager>(
    tee: &mut T,
    session_id: u64,
) -> Result<(), DebugStatusResponse> {
    let alias = "debugd:keymint-aes";
    let generate_payload =
        encode_keymint_generate_key(1, alias, KeyMintAlgorithm::AesGcm, false, 0, None)
            .map_err(|_| invalid_request_response("KeyMint AES generate encode failed"))?;
    let generated = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_GENERATE_KEY,
            payload: generate_payload,
        })
        .await;
    if generated.status != 0 {
        return Err(DebugStatusResponse {
            status: generated.status,
            message: "KeyMint AES generate failed".into(),
        });
    }
    let generated_key =
        decode_keymint_generated_key(&generated.response).map_err(|_| DebugStatusResponse {
            status: -8,
            message: "KeyMint AES generate returned invalid blob".into(),
        })?;
    if !generated_key.public_material.is_empty() {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint AES unexpectedly returned public material".into(),
        });
    }
    let plaintext = b"keymint aes-gcm plaintext";
    let aad = b"debugd aad";
    let (nonce, ciphertext) = keymint_operation(
        tee,
        session_id,
        KeyMintAlgorithm::AesGcm,
        KeyMintPurpose::Encrypt,
        &generated_key.opaque_blob,
        None,
        plaintext,
        aad,
    )
    .await?;
    let nonce = nonce.ok_or_else(|| DebugStatusResponse {
        status: -8,
        message: "KeyMint AES encrypt returned no nonce".into(),
    })?;
    let (_, decrypted) = keymint_operation(
        tee,
        session_id,
        KeyMintAlgorithm::AesGcm,
        KeyMintPurpose::Decrypt,
        &generated_key.opaque_blob,
        Some(&nonce),
        &ciphertext,
        aad,
    )
    .await?;
    if decrypted != plaintext {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint AES decrypt returned wrong plaintext".into(),
        });
    }
    keymint_delete_and_reject_smoke(
        tee,
        session_id,
        KeyMintAlgorithm::AesGcm,
        &generated_key.opaque_blob,
    )
    .await?;
    Ok(())
}

pub(super) async fn keymint_hmac_smoke<T: TeeManager>(
    tee: &mut T,
    session_id: u64,
) -> Result<(), DebugStatusResponse> {
    let alias = "debugd:keymint-hmac";
    let generate_payload =
        encode_keymint_generate_key(1, alias, KeyMintAlgorithm::HmacSha256, false, 0, None)
            .map_err(|_| invalid_request_response("KeyMint HMAC generate encode failed"))?;
    let generated = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_GENERATE_KEY,
            payload: generate_payload,
        })
        .await;
    if generated.status != 0 {
        return Err(DebugStatusResponse {
            status: generated.status,
            message: "KeyMint HMAC generate failed".into(),
        });
    }
    let generated_key =
        decode_keymint_generated_key(&generated.response).map_err(|_| DebugStatusResponse {
            status: -8,
            message: "KeyMint HMAC generate returned invalid blob".into(),
        })?;
    let (_, hmac) = keymint_operation(
        tee,
        session_id,
        KeyMintAlgorithm::HmacSha256,
        KeyMintPurpose::Sign,
        &generated_key.opaque_blob,
        None,
        b"debugd hmac",
        &[],
    )
    .await?;
    if hmac.len() != 32 {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint HMAC returned invalid output".into(),
        });
    }
    keymint_delete_and_reject_smoke(
        tee,
        session_id,
        KeyMintAlgorithm::HmacSha256,
        &generated_key.opaque_blob,
    )
    .await?;
    Ok(())
}

pub(super) async fn keymint_delete_and_reject_smoke<T: TeeManager>(
    tee: &mut T,
    session_id: u64,
    algorithm: KeyMintAlgorithm,
    opaque_blob: &[u8],
) -> Result<(), DebugStatusResponse> {
    let payload = encode_keymint_delete_key(opaque_blob)
        .map_err(|_| invalid_request_response("KeyMint delete encode failed"))?;
    let deleted = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_DELETE_KEY,
            payload,
        })
        .await;
    if deleted.status != 0 || decode_keymint_delete_key(&deleted.response).is_err() {
        return Err(DebugStatusResponse {
            status: if deleted.status == 0 {
                -8
            } else {
                deleted.status
            },
            message: "KeyMint delete failed".into(),
        });
    }
    let begin = encode_keymint_begin(
        algorithm,
        if algorithm == KeyMintAlgorithm::AesGcm {
            KeyMintPurpose::Encrypt
        } else {
            KeyMintPurpose::Sign
        },
        opaque_blob,
        None,
        None,
    )
    .map_err(|_| invalid_request_response("KeyMint deleted-key probe encode failed"))?;
    let rejected = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_BEGIN,
            payload: begin,
        })
        .await;
    if rejected.status != 0 {
        return Err(DebugStatusResponse {
            status: rejected.status,
            message: "KeyMint deleted-key probe transport failed".into(),
        });
    }
    if !matches!(
        decode_keymint_begin(&rejected.response),
        Err(bexos_trusty_client::protocol::TrustyWireError::SecureService(-33))
    ) {
        return Err(DebugStatusResponse {
            status: -8,
            message: "KeyMint did not reject deleted key with INVALID_KEY_BLOB".into(),
        });
    }
    Ok(())
}

pub(super) async fn keymint_operation<T: TeeManager>(
    tee: &mut T,
    session_id: u64,
    algorithm: KeyMintAlgorithm,
    purpose: KeyMintPurpose,
    opaque_blob: &[u8],
    nonce: Option<&[u8]>,
    input: &[u8],
    aad: &[u8],
) -> Result<(Option<[u8; 12]>, Vec<u8>), DebugStatusResponse> {
    let begin = encode_keymint_begin(algorithm, purpose, opaque_blob, nonce, None)
        .map_err(|_| invalid_request_response("KeyMint begin request encode failed"))?;
    let begun_response = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_BEGIN,
            payload: begin,
        })
        .await;
    if begun_response.status != 0 {
        return Err(DebugStatusResponse {
            status: begun_response.status,
            message: "KeyMint begin failed".into(),
        });
    }
    let begun =
        decode_keymint_begin(&begun_response.response).map_err(|error| DebugStatusResponse {
            status: -8,
            message: alloc::format!("KeyMint {algorithm:?} begin response: {error:?}"),
        })?;
    if !aad.is_empty() {
        let update = encode_keymint_update_aad(begun.handle, aad, None)
            .map_err(|_| invalid_request_response("KeyMint update-AAD encode failed"))?;
        let updated = tee
            .invoke(TeeInvokeRequest {
                session_id,
                command_id: KEYMINT_CMD_UPDATE_AAD,
                payload: update,
            })
            .await;
        if updated.status != 0 {
            return Err(DebugStatusResponse {
                status: updated.status,
                message: "KeyMint update-AAD failed".into(),
            });
        }
        decode_keymint_update_aad(&updated.response).map_err(|_| DebugStatusResponse {
            status: -8,
            message: "KeyMint update-AAD returned an invalid response".into(),
        })?;
    }
    let finish = encode_keymint_finish(begun.handle, input, None)
        .map_err(|_| invalid_request_response("KeyMint finish request encode failed"))?;
    let finished = tee
        .invoke(TeeInvokeRequest {
            session_id,
            command_id: KEYMINT_CMD_FINISH,
            payload: finish,
        })
        .await;
    if finished.status != 0 {
        return Err(DebugStatusResponse {
            status: finished.status,
            message: "KeyMint finish failed".into(),
        });
    }
    let output = decode_keymint_finish(&finished.response).map_err(|_| DebugStatusResponse {
        status: -8,
        message: "KeyMint finish returned an invalid response".into(),
    })?;
    Ok((begun.nonce, output))
}

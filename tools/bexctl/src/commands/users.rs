use super::*;
pub(super) fn run(c: &mut UnixDebugClient, cmd: UserCommand, f: Format) -> Result {
    let users = match cmd {
        UserCommand::List => c.list_users()?,
        UserCommand::Get { uid } => vec![c.get_user(uid)?],
        UserCommand::Create {
            uid,
            name,
            display_name,
            password,
        } => {
            c.create_user(
                uid,
                &name,
                display_name.as_deref().unwrap_or(&name),
                &password,
            )?;
            return Ok(());
        }
        UserCommand::Update {
            uid,
            name,
            display_name,
            disabled,
            current_password,
            new_password,
        } => {
            let display = display_name.as_deref().unwrap_or(&name);
            if let (Some(old), Some(new)) = (current_password, new_password) {
                c.replace_user_password(uid, &name, display, disabled, &old, &new)?;
            } else {
                c.update_user(uid, &name, display, disabled)?;
            }
            return Ok(());
        }
        UserCommand::Delete { uid } => {
            c.delete_user(uid)?;
            return Ok(());
        }
        UserCommand::Unlock { uid, password } => {
            c.unlock_user(uid, &password)?;
            return Ok(());
        }
        UserCommand::Lock { uid } => {
            c.lock_user(uid)?;
            return Ok(());
        }
    };
    let rows = users
        .into_iter()
        .map(|user| {
            json!({
                "uid": user.uid,
                "name": user.name,
                "display_name": user.display_name,
                "disabled": user.disabled,
                "unlocked": user.unlocked,
                "home": user.home_path,
            })
        })
        .collect();
    output::records(
        f,
        &[
            ("uid", "UID"),
            ("name", "NAME"),
            ("display_name", "DISPLAY NAME"),
            ("disabled", "DISABLED"),
            ("unlocked", "UNLOCKED"),
            ("home", "HOME"),
        ],
        rows,
    )?;
    Ok(())
}

//! The Linux half of the label-print program: what systemd holds under one
//! identity, in the system and per-user managers.
//!
//! `systemctl show` reads a unit without privilege, so this asks for none.
//! A manager that refuses the read says so; it never silently becomes an
//! absent unit.
pub(super) const BRANCH: &str = "\
elif [ \"$os\" = Linux ]; then
  case \"$scope\" in
    system) domains='system' ;;
    user)   domains='user' ;;
    *)      domains='user system' ;;
  esac
  properties='LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,ExecStart,Restart,Triggers,TriggeredBy,PartOf'
  for domain in $domains; do
    if [ \"$domain\" = system ]; then
      block=$(/usr/bin/systemctl show \"$label\" --property=\"$properties\" 2>&1)
    else
      runtime=\"/run/user/$uid\"
      block=$(/usr/bin/env XDG_RUNTIME_DIR=\"$runtime\" DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" /usr/bin/systemctl --user show \"$label\" --property=\"$properties\" 2>&1)
    fi
    read_code=$?
    if [ \"$read_code\" -ne 0 ]; then
      read_detail=$(printf '%s\\n' \"$block\" |
        /usr/bin/awk 'NR == 1 { gsub(/[\\t\\r]/, \" \"); print substr($0, 1, 300); exit }')
      case \"$block\" in
        *'Access denied'*|*'Permission denied'*|*'Interactive authentication required'*|\\
        *'password is required'*|*'not allowed to execute'*)
          if [ -z \"$read_detail\" ]; then read_detail='the manager refused the read without saying why'; fi
          printf 'STADO_LABEL_READ_REFUSED\\t%s\\t%s\\t%s\\n' \"$domain\" \"$read_code\" \"$read_detail\"
          continue ;;
      esac
      if [ -z \"$read_detail\" ]; then read_detail='systemctl show failed without detail'; fi
      printf 'STADO_LABEL_READ_FAILURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$read_code\" \"$read_detail\"
      continue
    fi
    if printf '%s\\n' \"$block\" | /usr/bin/awk -F= '$1 == \"LoadState\" && $2 == \"not-found\" { found=1 } END { exit !found }'; then
      continue
    fi
    found=yes
    printf 'STADO_LABEL_DOMAIN\\t%s\\n' \"$domain\"
    printf '%s\\n' \"$block\" | /usr/bin/awk -F= '
      $1 == \"MainPID\" { printf \"STADO_LABEL_FIELD\\tpid\\t%s\\n\", $2 }
      $1 == \"SubState\" { printf \"STADO_LABEL_FIELD\\tstate\\t%s\\n\", $2 }
      $1 == \"UnitFileState\" { printf \"STADO_LABEL_FIELD\\tunit file state\\t%s\\n\", $2 }
      $1 == \"FragmentPath\" { printf \"STADO_LABEL_FIELD\\tpath\\t%s\\n\", $2 }
      $1 == \"ExecStart\" { line=$0; sub(/^[^=]*=/, \"\", line); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", line }
      $1 == \"Restart\" { printf \"STADO_LABEL_FIELD\\trestart\\t%s\\n\", $2 }
      $1 == \"Triggers\" { printf \"STADO_LABEL_FIELD\\ttriggers\\t%s\\n\", $2 }
      $1 == \"TriggeredBy\" { printf \"STADO_LABEL_FIELD\\ttriggered by\\t%s\\n\", $2 }
      $1 == \"PartOf\" { printf \"STADO_LABEL_FIELD\\tpart of\\t%s\\n\", $2 }'
    main_pid=$(printf '%s\\n' \"$block\" | /usr/bin/awk -F= '$1 == \"MainPID\" { print $2; exit }')
    case \"$main_pid\" in
      ''|0|*[!0-9]*) ;;
      *)
        process_start=$(/usr/bin/ps -p \"$main_pid\" -o lstart= 2>/dev/null |
          /usr/bin/awk '{$1=$1; print; exit}')
        image=$(/usr/bin/readlink \"/proc/$main_pid/exe\" 2>/dev/null || true)
        mapped_device=$(/usr/bin/stat -Lc '%d' \"/proc/$main_pid/exe\" 2>/dev/null || true)
        mapped_inode=$(/usr/bin/stat -Lc '%i' \"/proc/$main_pid/exe\" 2>/dev/null || true)
        identity_error='process image could not be opened'
        image_digest=''
        opened_device=''
        opened_inode=''
        if [ -n \"$image\" ] && [ -n \"$mapped_device\" ] && [ -n \"$mapped_inode\" ] &&
           exec 9<\"/proc/$main_pid/exe\"; then
          opened_device=$(/usr/bin/stat -Lc '%d' /dev/fd/9 2>/dev/null || true)
          opened_inode=$(/usr/bin/stat -Lc '%i' /dev/fd/9 2>/dev/null || true)
          if [ \"$opened_device\" = \"$mapped_device\" ] &&
             [ \"$opened_inode\" = \"$mapped_inode\" ]; then
            image_digest=$(/usr/bin/openssl dgst -sha256 -r /dev/fd/9 2>/dev/null)
            image_digest=${image_digest%% *}
            after_device=$(/usr/bin/stat -Lc '%d' /dev/fd/9 2>/dev/null || true)
            after_inode=$(/usr/bin/stat -Lc '%i' /dev/fd/9 2>/dev/null || true)
            if [ \"$after_device\" != \"$opened_device\" ] ||
               [ \"$after_inode\" != \"$opened_inode\" ]; then
              image_digest=''
              identity_error='opened image changed during hashing'
            fi
          else
            identity_error='mapped image no longer names the opened inode'
          fi
          exec 9<&-
        fi
        if [ \"$domain\" = system ]; then
          current_pid=$(/usr/bin/systemctl show \"$label\" --property=MainPID --value 2>/dev/null || true)
        else
          current_pid=$(/usr/bin/env XDG_RUNTIME_DIR=\"$runtime\" DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" /usr/bin/systemctl --user show \"$label\" --property=MainPID --value 2>/dev/null || true)
        fi
        current_start=$(/usr/bin/ps -p \"$current_pid\" -o lstart= 2>/dev/null |
          /usr/bin/awk '{$1=$1; print; exit}')
        current_image=$(/usr/bin/readlink \"/proc/$current_pid/exe\" 2>/dev/null || true)
        current_device=$(/usr/bin/stat -Lc '%d' \"/proc/$current_pid/exe\" 2>/dev/null || true)
        current_inode=$(/usr/bin/stat -Lc '%i' \"/proc/$current_pid/exe\" 2>/dev/null || true)
        case \"$image_digest\" in
          [0-9a-f][0-9a-f]*)
            if [ \"$current_pid\" = \"$main_pid\" ] &&
               [ -n \"$process_start\" ] && [ \"$current_start\" = \"$process_start\" ] &&
               [ -n \"$image\" ] && [ \"$current_image\" = \"$image\" ] &&
               [ \"$current_device\" = \"$opened_device\" ] &&
               [ \"$current_inode\" = \"$opened_inode\" ]; then
              printf 'STADO_LABEL_FIELD\\tprocess start\\t%s\\n' \"$process_start\"
              printf 'STADO_LABEL_FIELD\\tprocess executable\\t%s\\n' \"$image\"
              printf 'STADO_LABEL_FIELD\\tprocess device\\t%s\\n' \"$opened_device\"
              printf 'STADO_LABEL_FIELD\\tprocess inode\\t%s\\n' \"$opened_inode\"
              printf 'STADO_LABEL_FIELD\\tprocess sha256\\t%s\\n' \"$image_digest\"
            else
              printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\tprocess changed during identity capture\\n'
            fi
            ;;
          *) printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\t%s\\n' \"$identity_error\" ;;
        esac
        ;;
    esac
    break
  done
";

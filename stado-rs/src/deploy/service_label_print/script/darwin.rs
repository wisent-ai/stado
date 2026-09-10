//! The Darwin half of the label-print program: what launchd holds under one
//! identity, in the three domains a Mac can hold it in.
//!
//! Reads only. `launchctl print` needs no privilege to read a domain, so no
//! read here asks for one: a refusal is the exception, and it is reported as
//! a refusal rather than turned into absence.
pub(super) const BRANCH: &str = "\
if [ \"$os\" = Darwin ]; then
  case \"$scope\" in
    system) domains='system' ;;
    user)   domains=\"user/$uid gui/$uid\" ;;
    *)      domains=\"system user/$uid gui/$uid\" ;;
  esac
  for domain in $domains; do
    block=$(/bin/launchctl print \"$domain/$label\" 2>&1)
    read_code=$?
    if [ \"$read_code\" -ne 0 ]; then
      read_detail=$(printf '%s\\n' \"$block\" |
        /usr/bin/awk 'NR == 1 { gsub(/[\\t\\r]/, \" \"); print substr($0, 1, 300); exit }')
      case \"$block\" in
        *'Could not find service'*|*'Could not find domain'*|*'No such process'*) continue ;;
        *'Operation not permitted'*|*'Permission denied'*|*'not permitted'*|\\
        *'password is required'*|*'not allowed to execute'*|*'must be root'*)
          if [ -z \"$read_detail\" ]; then read_detail='the domain refused the read without saying why'; fi
          printf 'STADO_LABEL_READ_REFUSED\\t%s\\t%s\\t%s\\n' \"$domain\" \"$read_code\" \"$read_detail\"
          continue ;;
      esac
      if [ -z \"$read_detail\" ]; then read_detail='launchctl print failed without detail'; fi
      printf 'STADO_LABEL_READ_FAILURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$read_code\" \"$read_detail\"
      continue
    fi
    if [ -z \"$block\" ]; then
      printf 'STADO_LABEL_READ_FAILURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$read_code\" \\
        'the init system answered for this domain with nothing'
      continue
    fi
    found=yes
    printf 'STADO_LABEL_DOMAIN\\t%s\\n' \"$domain\"
    printf '%s\\n' \"$block\" | /usr/bin/awk -F' = ' '
      # Only the fields of the job itself: launchd indents this block at one
      # tab and every nested dictionary deeper, and those nested dictionaries
      # carry a state of their own. Reading them made a job whose top-level
      # state is running report the state of an endpoint instead, and a
      # collector then published a live daemon as failed.
      /^\\t[^\\t]/ { key=$1; sub(/^[ \\t]+/, \"\", key); sub(/[ \\t]+$/, \"\", key) }
      /^\\t[^\\t]/ && !seen[key]++ &&
      (key == \"pid\" || key == \"state\" || key == \"last exit code\" || key == \"runs\" || key == \"path\" || key == \"stdout path\" || key == \"stderr path\") {
        value=$2
        sub(/^[ \\t]+/, \"\", value); sub(/[ \\t]+$/, \"\", value)
        printf \"STADO_LABEL_FIELD\\t%s\\t%s\\n\", key, value
      }'
    printf '%s\\n' \"$block\" | /usr/bin/awk '
      /^[ \\t]*program[ \\t]*=/ { line=$0; sub(/^[^=]*=[ \\t]*/, \"\", line); printf \"STADO_LABEL_FIELD\\tprogram\\t%s\\n\", line }
      /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", argv; next }
      collecting { line=$0; sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }'
    printf '%s\\n' \"$block\" | /usr/bin/awk '
      /^[ \\t]*environment[ \\t]*=[ \\t]*\\{/ { collecting=1; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; next }
      collecting {
        line=$0
        sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line)
        split(line, pair, /[ \\t]+=>[ \\t]+/)
        key=pair[1]
        if (key == \"WC_STORAGE_BACKEND\" || key == \"WC_LOCAL_STORAGE_PATH\" ||
            key == \"WC_BACKUP_STORAGE_BACKEND\" ||
            key == \"WC_BACKUP_LOCAL_STORAGE_PATH\" || key == \"STADO_CONFIG\") {
          value=line
          sub(/^[^=]*=>[ \\t]*/, \"\", value)
          gsub(/\\t/, \" \", value)
          printf \"STADO_LABEL_ENV\\t%s\\t%s\\n\", key, value
        }
      }'
    launch_pid=$(printf '%s\\n' \"$block\" | /usr/bin/awk -F' = ' '$1 ~ /^[ \\t]*pid$/ { print $2; exit }')
    if [ -n \"$launch_pid\" ] && [ -x /usr/sbin/lsof ]; then
      process_start=$(/bin/ps -p \"$launch_pid\" -o lstart= 2>/dev/null |
        /usr/bin/awk '{$1=$1; print; exit}')
      mapping=$(/usr/sbin/lsof -a -p \"$launch_pid\" -d txt -F pDsikn 2>/dev/null |
        /usr/bin/awk '
          substr($0,1,1) == \"p\" { if (seen) exit; seen=1 }
          seen && substr($0,1,1) ~ /[Dsin]/ { print }
          seen && substr($0,1,1) == \"n\" { exit }')
      image=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"n\" { print substr($0,2); exit }')
      mapped_device=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"D\" { print substr($0,2); exit }')
      mapped_inode=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"i\" { print substr($0,2); exit }')
      identity_error='process image could not be opened'
      image_digest=''
      opened_device=''
      opened_inode=''
      if [ -n \"$image\" ] && [ -n \"$mapped_device\" ] && [ -n \"$mapped_inode\" ] &&
         exec 9<\"$image\"; then
        opened_identity=$(/usr/bin/stat -f '%d %i' <&9 2>/dev/null || true)
        opened_device=${opened_identity%% *}
        opened_inode=${opened_identity#* }
        mapped_device_decimal=$((mapped_device))
        if [ -n \"$opened_device\" ] && [ -n \"$opened_inode\" ] &&
           [ \"$opened_device\" -eq \"$mapped_device_decimal\" ] &&
           [ \"$opened_inode\" -eq \"$mapped_inode\" ]; then
          image_digest=$(/usr/bin/openssl dgst -sha256 -r <&9 2>/dev/null)
          image_digest=${image_digest%% *}
          after_identity=$(/usr/bin/stat -f '%d %i' <&9 2>/dev/null || true)
          after_device=${after_identity%% *}
          after_inode=${after_identity#* }
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
      current=$($launch print \"$domain/$label\" 2>/dev/null || true)
      current_pid=$(printf '%s\\n' \"$current\" |
        /usr/bin/awk -F' = ' '$1 ~ /^[ \\t]*pid$/ { print $2; exit }')
      current_start=$(/bin/ps -p \"$current_pid\" -o lstart= 2>/dev/null |
        /usr/bin/awk '{$1=$1; print; exit}')
      current_mapping=$(/usr/sbin/lsof -a -p \"$current_pid\" -d txt -F pDsikn 2>/dev/null |
        /usr/bin/awk '
          substr($0,1,1) == \"p\" { if (seen) exit; seen=1 }
          seen && substr($0,1,1) ~ /[Dsin]/ { print }
          seen && substr($0,1,1) == \"n\" { exit }')
      case \"$image_digest\" in
        [0-9a-f][0-9a-f]*)
          if [ \"$current_pid\" = \"$launch_pid\" ] &&
             [ -n \"$process_start\" ] && [ \"$current_start\" = \"$process_start\" ] &&
             [ -n \"$mapping\" ] && [ \"$current_mapping\" = \"$mapping\" ]; then
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
    elif [ -n \"$launch_pid\" ]; then
      printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\tlsof is unavailable\\n'
    fi
    if [ -x /usr/bin/log ]; then
      {
        /usr/bin/log show --last 1h --style compact --predicate \"$predicate\" 2>&1
        printf 'STADO_LABEL_EVENT_EXIT\\t%s\\n' \"$?\"
      } |
        STADO_LABEL=\"$label\" STADO_QUALIFIED=\"$domain/$label\" LC_ALL=C /usr/bin/awk '
          function complete_identity_field(line, identity) {
            return index(line, \"(\" identity \")\") > 0 ||
                   index(line, \"[\" identity \"]\") > 0 ||
                   index(line, \"[\" identity \":]\") > 0 ||
                   index(line, \"[\" identity \" [\") > 0
          }
          BEGIN {
            label=ENVIRON[\"STADO_LABEL\"]
            qualified=ENVIRON[\"STADO_QUALIFIED\"]
          }
          index($0, \"STADO_LABEL_EVENT_EXIT\\t\") == 1 {
            status=substr($0, length(\"STADO_LABEL_EVENT_EXIT\\t\") + 1)
            saw_status=1
            next
          }
          {
            detail=substr($0, 1, 512)
            if (complete_identity_field($0, qualified) ||
                complete_identity_field($0, label)) {
              count++
              events[(count - 1) % 12]=substr($0, 1, 2048)
            }
          }
          END {
            if (!saw_status) {
              printf \"STADO_LABEL_EVENT_STATUS\\terror: log reader returned no status\\n\"
            } else if (status != 0) {
              gsub(/\\t/, \" \", detail)
              printf \"STADO_LABEL_EVENT_STATUS\\terror %s: %s\\n\", status, detail
            } else {
              first=count > 12 ? count - 11 : 1
              for (number=first; number <= count; number++) {
                event=events[(number - 1) % 12]
                gsub(/\\t/, \" \", event)
                printf \"STADO_LABEL_EVENT\\t%s\\n\", event
              }
              printf \"STADO_LABEL_EVENT_STATUS\\tok\\n\"
            }
          }'
    else
      printf 'STADO_LABEL_EVENT_STATUS\\tunavailable: /usr/bin/log is absent\\n'
    fi
    break
  done
";

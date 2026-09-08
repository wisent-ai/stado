/// `service ensure`: the unit this host should be running, installed only
/// where it is not already what it should be.
///
/// Three differences from [`DEPLOY_BODY`], each one an incident:
///
/// - It is idempotent. `deploy` refuses a unit that is already declared and
///   bootstraps unconditionally otherwise, so there is no command an operator
///   can run twice, or run from a script, to assert what a host must be
///   running. This one reads what is there first and reports
///   `already_correct` having touched nothing.
/// - It installs into the domain that exists. The prelude's
///   [`NO_DOMAIN_SYSTEM`] fallback has already chosen
///   `/Library/LaunchDaemons` on an ssh login with no Aqua session, which is
///   the case `deploy` fails on with `Could not switch to audit session ...
///   Operation not permitted`, having installed nothing.
/// - It compares the plist, launchd's retained Program and argument vector,
///   and the running executable. A differing retained definition is reloaded
///   only after executable and rendered-unit preflight, with a genuinely
///   distinct prior unit restored if activation fails and launchd's readback
///   verified on success.
///
/// There is deliberately no fallback to `launchctl submit` or to a bare
/// background process. Those two are how a host comes to run a program no
/// unit owns, which is the state `list --unowned` exists to find and this
/// command exists to end.
pub(super) const ENSURE_BODY_HEAD: &str = "program=@PROGRAM@
argv=@ARGV@
# The staged unit is removed inline, on every path, and NO `trap` is installed
# for it. `host_channel::PostCondition::arm` arms the end-state probe as an EXIT
# trap before this body runs, and a second `trap ... EXIT` here replaces it: a
# create pass then wrote the plist, bootstrapped it, left launchd running it
# with a live pid, and still failed with `postcondition unobserved`, because the
# probe that would have confirmed the success had been unhooked by the cleanup.
staged=''
stado_loaded_identity() {
  loaded_program=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*program[[:space:]]*$/ { print $2; exit }')
  loaded_arguments_rc=0
  loaded_argv=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk '
    /^[[:space:]]*arguments[[:space:]]*=[[:space:]]*\\{/ { seen=1; collecting=1; argv=\"\"; next }
    collecting && /^[[:space:]]*\\}/ { complete=1; sub(/^ /, \"\", argv); print argv; exit }
    collecting { line=$0; sub(/^[[:space:]]+/, \"\", line); sub(/[[:space:]]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }
    END { if (!seen) exit 3; if (!complete) exit 4 }') || loaded_arguments_rc=$?
  loaded_program=$(printf '%s' \"$loaded_program\" | /usr/bin/sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
  loaded_arguments_valid=yes
  case \"$loaded_arguments_rc\" in
    0) loaded_argv=$(printf '%s' \"$loaded_argv\" | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//') ;;
    3) loaded_argv=\"$loaded_program\" ;;
    *) loaded_arguments_valid=no; loaded_argv='' ;;
  esac
}
# Whether the process launchd or systemd reports under this unit executes the
# declared program. Sets `running` and `serves`.
#
# `comm` is the image the kernel runs, and for a program that is a launcher it
# is the launcher's exec target: `bin/start-web` execs node, so every web unit
# reports `node` and equality with the program fails for all of them. On
# 2026-09-05 that refused the reload of two running sites with `pid 6678
# executes [node]; expected [.../current/darwin-arm/bin/start-web]` and
# restarted every healthy web unit on each ensure pass, because the idle check
# read the same `no`. A launcher's process still runs the product: its image,
# an argument, or its working directory lies under the product root that the
# `current` link belongs to. That is the evidence accepted here, and it is one
# rule for the idle check and the post-reload verification, so one process
# cannot be `already_correct` before a reload and a verification failure after
# it. A program outside a `current` tree keeps the exact comparison: the
# control-plane job that went on executing the shared global binary its plist
# no longer named is the case that comparison exists for.
stado_process_serves() {
  running=''
  serves=no
  [ -n \"$1\" ] || return 0
  running=$(/bin/ps -p \"$1\" -o comm= 2>/dev/null)
  case \"$running\" in
    \"$program\") serves=yes; return 0 ;;
  esac
  case \"$program\" in
    */current/*) ;;
    *) return 0 ;;
  esac
  product_root=\"${program%%/current/*}/\"
  case \"$running\" in
    \"$product_root\"*) serves=yes; return 0 ;;
  esac
  command=$(/bin/ps -p \"$1\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
  case \" $command\" in
    *\" $product_root\"*|*\"=$product_root\"*) serves=yes; return 0 ;;
  esac
  if [ \"$os\" = Darwin ]; then
    cwd=$(/usr/sbin/lsof -a -p \"$1\" -d cwd -Fn 2>/dev/null | /usr/bin/sed -n 's/^n//p' | /usr/bin/head -n 1)
  else
    cwd=$(/usr/bin/readlink \"/proc/$1/cwd\" 2>/dev/null)
  fi
  case \"$cwd/\" in
    \"$product_root\"*) serves=yes ;;
  esac
}
bail() {
  if [ -n \"$staged\" ]; then /bin/rm -f \"$staged\" \"$staged.rendered\"; fi
  say 'ensure_failed' \"$1\"
  exit 0
}
if [ ! -f \"$program\" ]; then
  say 'program_missing' \"$program\"
  exit 0
fi
if [ ! -x \"$program\" ]; then
  /bin/chmod u+x \"$program\" || {
    say 'program_not_executable' \"$program\"
    exit 0
  }
fi
# What the unit on disk says it runs, as the one-line spelling `plan_deploy`
# renders, so 'the unit already runs this' is a comparison of two spellings of
# one list rather than a guess.
#
# Exactly PlistBuddy's own array framing is dropped, by matching those two
# lines. `service show`'s character-class filter keeps the arguments only
# because PlistBuddy indents them, and the whole of this command turns on the
# comparison being exact: a readback that lost one argument would make a unit
# this command installed itself read as declaring something else on the very
# next run, and the answer would be `conflict` forever.
declared_argv=''
had_unit=no
if [ \"$os\" = \"Darwin\" ]; then
  if [ -f \"$unit_path\" ]; then
    declared_argv=$(stado_unit_argv \"$unit_path\")
  fi
  stado_launchd_state
  had_unit=\"$pc_loaded\"
  pid=\"$pc_pid\"
  stado_loaded_identity
else
  if [ -f \"$unit_path\" ]; then
    had_unit=yes
    declared_argv=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
  fi
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
declared_argv=$(printf '%s' \"$declared_argv\" | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//')
# The program the live process is executing, not the one the unit names: a
# unit pointing at a `current` link and a process that outlived the relink
# have the same declaration and different code.
stado_process_serves \"$pid\"
# Compare the whole desired unit, including its environment, on both init
# systems. A loaded launchd definition can outlive a removed plist, but the
# desired declaration is still complete enough to render and safely reload it.
rendered=''
if [ -f \"$unit_path\" ] || { [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ]; }; then
  staged=\"$HOME/.stado/$unit.ensure.$$\"
  if [ \"$os\" = Linux ]; then
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  elif [ \"$domain\" = system ]; then
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_DAEMON_UNIT@
@HEREDOC@
  else
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
  fi
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  account=$(/usr/bin/id -un)
  /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
  rendered=\"$staged.rendered\"
  if [ \"$os\" = Darwin ]; then
    rc=0
    detail=$(/usr/bin/plutil -lint \"$rendered\" 2>&1) || rc=$?
    if [ \"$rc\" -ne 0 ]; then bail \"plutil preflight exited $rc: ${detail:-no detail}\"; fi
  fi
fi
stado_install_unit() {
  if [ \"$os\" = Darwin ] && [ \"$domain\" = system ]; then
    /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \"$1\" \"$unit_path.stado-ensure.$$\" \
      && /usr/bin/sudo -n /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  elif [ \"$os\" = Linux ] && [ \"$scope\" = system ]; then
    stado_root /usr/bin/install -m 644 -o root -g root \"$1\" \"$unit_path.stado-ensure.$$\" \
      && stado_root /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  else
    /bin/cp \"$1\" \"$unit_path.stado-ensure.$$\" \
      && /bin/chmod u=rw,go= \"$unit_path.stado-ensure.$$\" \
      && /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  fi
}
stado_activate_definition() {
  activation_failure=''
  if [ \"$os\" = Darwin ]; then
    stado_launchd_state
    if [ \"$pc_loaded\" = yes ]; then
      activation_detail=$($launch bootout \"$domain/$unit\" 2>&1)
      activation_rc=$?
      if [ \"$activation_rc\" -ne 0 ]; then
        activation_failure=\"launchctl bootout exited $activation_rc: ${activation_detail:-no detail}\"
        return 1
      fi
      attempts=0
      while $launch print \"$domain/$unit\" >/dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ \"$attempts\" -ge 150 ]; then
          activation_failure=\"launchctl bootout exited 0 but $domain/$unit remained loaded\"
          return 1
        fi
        /bin/sleep 0.1
      done
    fi
    # A disabled service is what `stado service stop` and the release agent's
    # `stop_legacy` leave behind, and `bootstrap` refuses it with `Bootstrap
    # failed: 5: Input/output error` - the create path below already enables
    # before it bootstraps, and this path did not. On 2026-09-06 that refused
    # the one command that could give charless-mac-mini its Skarbiec unit back
    # after the release path abandoned the stable bind, and then failed the
    # rollback with the same error, thirteen hours into an outage.
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    activation_detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"launchctl bootstrap exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
  else
";

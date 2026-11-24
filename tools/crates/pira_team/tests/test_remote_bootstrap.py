"""Installed-only discovery; synthetic caller context and inert backend, no downloads."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time
import unittest
from test_remote_fixture import RemoteFixture

def team_version(fixture):
 return subprocess.check_output([str(fixture.bin),"--version"],env=fixture.env,text=True,timeout=5).strip().split()[1]

SSH=r'''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
sys.stderr.write(os.environ.get("TEAM_BOOTSTRAP_STDERR", ""))
wire=sys.stdin.read(); data=json.loads(wire)
with open(os.environ["TEAM_REMOTE_TRACE"],"a") as out:
 out.write(json.dumps({"argv":sys.argv[1:],"wire":wire})+"\n")
root=Path(os.environ["TEAM_BOOTSTRAP_HOME"])
if "__remote-info" in sys.argv[-1]:
 print(json.dumps({"version":os.environ["TEAM_BOOTSTRAP_VERSION"],"protocol":2,"setup":1,"exe":str(root/"installed Team with spaces"),"user":"fixture-user","home":str(root)}))
elif "__remote-setup" in sys.argv[-1]:
 if os.environ.get("TEAM_BOOTSTRAP_FAIL"):
  print(json.dumps({"error":"Codex CLI not found; install/login Codex on the server"}))
 else:
  print(json.dumps({"user":"fixture-user","endpoint_id":"b"*64,"exe":str(root/"installed Team with spaces"),"backend":str(root/"codex with spaces"),"store":str(root/"runs")}))
else:
 grant=data["grant"]
 if grant["args"]==["capabilities"]:
  result={"protocol":2,"scope":"root-main-host-pinned","capabilities":["signed-delegation","read","durable-submit"]}
 else:
  run="remote-"+grant["nonce"]
  result={"accepted":True,"request_id":grant["request_id"],"run_id":run,"supervisor":"unknown","manifest":{"run_id":run,"creator_main_session_id":grant["issuer"],"parent_run_id":None,"revision":1,"status":"completed"}}
 print(json.dumps({"version":2,"ok":True,"result":result}))
'''

@unittest.skipUnless(os.name=="posix","POSIX SSH fixture")
class InstalledDiscoveryTests(unittest.TestCase):
 def setUp(self):
  self.f=RemoteFixture(self)
  self.f.env["PIRA_TEAM_DIR"]=str(self.f.root/"local store")
  self.f.env["TEAM_BOOTSTRAP_HOME"]=str(self.f.root)
  self.f.env["TEAM_BOOTSTRAP_VERSION"]=team_version(self.f)
  (self.f.root/"ssh").write_text(SSH.replace("#!/usr/bin/env python3","#!"+sys.executable))
 def launch(self,*extra):
  return self.f.invoke("run","--host","fixture-alias","--task","inert fixture","--completion-gate","Return fixture","--model","test-model","--effort","high","--detach",*extra)
 def test_only_host_discovers_installed_team_and_reuses_local_authority(self):
  for _ in range(2):
   result=self.launch(); self.assertEqual(result.returncode,0,result.stderr)
  exchanges=self.f.exchanges(); setups=[json.loads(x["wire"]) for x in exchanges if "__remote-setup" in x["argv"][-1]]
  self.assertEqual(setups[0]["public"],setups[1]["public"])
  for x in exchanges:
   for setting in ("StrictHostKeyChecking=yes","ForwardAgent=no","ClearAllForwardings=yes","ControlPath=none"):
    self.assertIn(setting,x["argv"])
   self.assertNotIn("python",x["argv"][-1]); self.assertNotIn("curl",x["argv"][-1])
   self.assertNotIn("payload",(json.loads(x["wire"]) or {}))
   self.assertNotIn("PARENT_TRANSCRIPT_MUST_NOT_BE_FORWARDED",x["wire"])
  discovery=exchanges[0]["argv"][-1]
  self.assertIn("command -v pira_team",discovery)
  self.assertIn('$HOME/.local/bin/pira_team',discovery)
 def test_successful_discovery_setup_and_signed_requests_share_diagnostics(self):
  banner="fixture login diagnostic\n"
  self.f.env["TEAM_BOOTSTRAP_STDERR"]=banner
  for _ in range(2):
   result=self.launch();self.assertEqual(result.returncode,0,result.stderr)
   self.assertEqual(result.stderr.count(banner),1,result.stderr)
   self.assertRegex(json.loads(result.stdout)["run_id"],r"^remote-[0-9a-f]{64}$")
  phases=[entry["argv"][-1].split()[-1] for entry in self.f.exchanges()]
  self.assertEqual(phases,["__remote-info","__remote-setup","__remote-serve","__remote-serve"]*2)
 def test_failed_setup_repeats_identical_diagnostics(self):
  banner="fixture login diagnostic\n"
  self.f.env["TEAM_BOOTSTRAP_STDERR"]=banner
  path=self.f.root/"ssh"
  path.write_text(path.read_text().replace('elif "__remote-setup" in sys.argv[-1]:',
      'elif "__remote-setup" in sys.argv[-1]:\n sys.exit(255)'))
  result=self.launch();self.assertNotEqual(result.returncode,0)
  self.assertEqual(result.stderr.count(banner),2,result.stderr)
  self.assertIn("SSH process failed",result.stderr)
  self.assertEqual(len(self.f.exchanges()),2)
 def test_standard_location_fallback_works_with_empty_path_and_no_python(self):
  result=self.launch();self.assertEqual(result.returncode,0,result.stderr)
  command=self.f.exchanges()[0]["argv"][-1]
  home=self.f.root/"standard home";installed=home/".local/bin/pira_team"
  installed.parent.mkdir(parents=True);shutil.copy2(self.f.bin,installed)
  empty=self.f.root/"empty PATH";empty.mkdir()
  # A non-executable namesake is not a PATH program and must not hide system OpenSSH.
  (empty/"ssh-keygen").write_text("not executable")
  result=subprocess.run(["/bin/sh","-c",command],env={"HOME":str(home),"PATH":str(empty)},capture_output=True,text=True,timeout=15)
  self.assertEqual(result.returncode,0,result.stderr)
  self.assertEqual(json.loads(result.stdout)["exe"],str(installed))
 def test_missing_installed_team_fails_without_creating_home_or_metadata(self):
  result=self.launch();self.assertEqual(result.returncode,0,result.stderr)
  command=self.f.exchanges()[0]["argv"][-1]
  home=self.f.root/"missing-home"
  result=subprocess.run(["/bin/sh","-c",command],env={"HOME":str(home),"PATH":""},capture_output=True,text=True,timeout=15)
  self.assertEqual(result.returncode,127);self.assertIn("run PIRA setup",result.stderr)
  self.assertFalse(home.exists())
 def test_explicit_installed_selection_with_spaces_is_quoted_not_copied(self):
  selected="/installed Team with spaces"
  result=self.launch("--remote-exe",selected);self.assertEqual(result.returncode,0,result.stderr)
  self.assertEqual(self.f.exchanges()[0]["argv"][-1],"exec "+shlex.quote(selected)+" __remote-info")
 def test_account_mismatch_stops_before_enrollment(self):
  result=self.launch("--remote-user","forged");self.assertNotEqual(result.returncode,0)
  self.assertIn("authenticated SSH account",result.stderr);self.assertEqual(len(self.f.exchanges()),1)
 def test_missing_backend_has_actionable_error_and_no_submission(self):
  self.f.env["TEAM_BOOTSTRAP_FAIL"]="1"
  result=self.launch();self.assertNotEqual(result.returncode,0);self.assertIn("Codex CLI not found",result.stderr)
  self.assertTrue(all("grant" not in (json.loads(x["wire"]) or {}) for x in self.f.exchanges()))
 def test_old_installed_team_is_rejected_without_setup_or_download(self):
  path=self.f.root/"ssh";path.write_text(path.read_text().replace('"setup":1','"setup":0'))
  result=self.launch();self.assertNotEqual(result.returncode,0);self.assertIn("update PIRA Team",result.stderr)
  self.assertEqual(len(self.f.exchanges()),1)
 def test_different_installed_version_is_rejected_before_enrollment(self):
  self.f.env["TEAM_BOOTSTRAP_VERSION"]="incompatible-fixture-version"
  result=self.launch();self.assertNotEqual(result.returncode,0)
  self.assertIn("update PIRA Team",result.stderr)
  self.assertEqual(len(self.f.exchanges()),1)
 def test_old_signer_fails_before_remote_enrollment_or_submission(self):
  signer=self.f.root/"ssh-keygen";signer.write_text("#!/bin/sh\necho 'usage: ssh-keygen -t rsa' >&2\nexit 1\n");signer.chmod(0o700)
  result=self.launch();self.assertNotEqual(result.returncode,0)
  self.assertIn("-Y sign/verify",result.stderr)
  self.assertEqual(len(self.f.exchanges()),1)
 def test_worker_cannot_discover_or_enroll_remote_host(self):
  self.f.env["PIRA_TEAM_CHILD"]=str(self.f.root/"not-a-run")
  result=self.launch();self.assertNotEqual(result.returncode,0);self.assertEqual(self.f.exchanges(),[])

@unittest.skipUnless(sys.platform=="linux","native installed metadata setup")
class InstalledMetadataTests(unittest.TestCase):
 def setUp(self):
  self.f=RemoteFixture(self)
  self.server=self.f.root/"installed Team with spaces";shutil.copy2(self.f.bin,self.server)
  self.key=self.f.generate_key("synthetic-public-key")
  self.public=" ".join(Path(str(self.key)+".pub").read_text().split()[:2])
  self.issuer="fixture-"+self.f.root.name
 def setup(self,issuer=None,**overrides):
  data={"version":team_version(self.f),"protocol":2,"issuer":issuer or self.issuer,"public":self.public,"backend":str(self.f.root/"codex"),"store":str(self.f.store)}
  data.update(overrides)
  result=subprocess.run([str(self.server),"__remote-setup"],input=json.dumps(data),env=self.f.env,capture_output=True,text=True,timeout=15)
  self.assertEqual(result.returncode,0,result.stderr);return json.loads(result.stdout)
 def test_metadata_is_idempotent_and_old_public_enrollment_is_untouched(self):
  old=Path(str(self.server)+".allowed-signers");old.write_text("unrelated old enrollment\n")
  before=self.server.read_bytes()
  first=self.setup();self.assertNotIn("error",first)
  self.assertEqual(first,self.setup())
  other=self.setup(self.issuer+"-other");self.assertNotEqual(first["endpoint_id"],other["endpoint_id"])
  self.assertEqual(self.server.read_bytes(),before)
  self.assertEqual(old.read_text(),"unrelated old enrollment\n")
 def capabilities(self,selected):
  now=int(time.time())
  endpoint={"host":"fixture-alias",**{key:selected[key] for key in ("user","endpoint_id","exe","backend","store")}}
  grant={"protocol":2,"scope":"root-main-host-pinned","issuer":self.issuer,"endpoint":endpoint,"op":"read",
         "request_id":"metadata-probe","run_request":None,"args":["capabilities"],"context":None,
         "issued_at":now,"expires_at":now+300,"nonce":"d"*64,"file":None,"artifact":False,"operation_id":None,"cwd":None,"target":None}
  envelope={"version":2,"grant":grant,"signature":self.f.sign(grant,self.key,"pira-team-remote-v2")}
  result=subprocess.run([str(self.server),"__remote-serve"],input=json.dumps(envelope),env=self.f.env,capture_output=True,text=True,timeout=15)
  self.assertEqual(result.returncode,0,result.stderr)
  self.assertTrue(json.loads(result.stdout)["ok"],result.stdout)
 def test_xdg_and_setup_selected_roots_are_used_for_setup_and_signed_lookup(self):
  for key in ("XDG_DATA_HOME","PIRA_TEAM_DIR"):
   with self.subTest(key=key):
    configured=self.f.root/key;configured.mkdir(mode=0o700)
    self.f.env.pop("PIRA_TEAM_DIR",None)
    self.f.env[key]=str(configured)
    selected=self.setup()
    self.assertNotIn("error",selected)
    root=configured/(".remote-control-v2" if key=="PIRA_TEAM_DIR" else "pira-team-control-v2")
    self.assertTrue((root/selected["endpoint_id"]/"endpoint.json").is_file())
    self.capabilities(selected)
 def test_relative_control_root_is_rejected_without_creating_state(self):
  self.f.env["XDG_DATA_HOME"]="relative-control-root"
  result=self.setup();self.assertIn("absolute",result["error"])
 def test_actual_account_ignores_user_and_home_environment(self):
  import pwd
  env={**self.f.env,"USER":"forged","HOME":"/forged"}
  result=subprocess.run([str(self.server),"__remote-info"],env=env,capture_output=True,text=True,timeout=15)
  self.assertEqual(result.returncode,0,result.stderr)
  info=json.loads(result.stdout);self.assertEqual(info["user"],pwd.getpwuid(os.geteuid()).pw_name)
  self.assertEqual(info["home"],str(Path(pwd.getpwuid(os.geteuid()).pw_dir).resolve()))

if __name__=="__main__":unittest.main()

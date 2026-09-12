import unittest,tempfile,pathlib,subprocess,json,os
SCRIPT=pathlib.Path(__file__).with_name('collect-static.py')
class CollectTests(unittest.TestCase):
 def test_workspace_boundaries_and_links(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);work=root/'work';state=root/'state';work.mkdir();state.mkdir();out=work/'test1'/'dist';out.mkdir(parents=True);(out/'index.html').write_text('hello')
   def run(directory):return subprocess.run(['python3',str(SCRIPT),str(work),str(state),'current',directory],capture_output=True,text=True)
   result=run('test1/dist');self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)['files'][0]['path'],'index.html')
   self.assertNotEqual(run('../work/test1/dist').returncode,0)
   (work/'link').symlink_to(out,target_is_directory=True);self.assertNotEqual(run('link').returncode,0)
   (out/'leak.txt').symlink_to('/etc/passwd');self.assertNotEqual(run('test1/dist').returncode,0)
if __name__=='__main__':unittest.main()

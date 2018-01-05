use jsonrpc_core::*;
use jsonrpc_core::serde_json::{Map};
use jsonrpc_tcp_server::*;
use std::u32;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::result::Result as StdResult;
use concurrent_hashmap::*;
use uuid::*;
use reqwest;
use schedule_recv::periodic_ms;
use std::thread;
use num_bigint::*;
use num_integer::*;
use mithril::byte_string;
use mithril::cryptonight::*;
use cryptonightlite;
use config::*;

enum HashType {
  Cryptonight,
  CryptonightLite,
}

fn cn_hash(input: Vec<u8>, hash_type: HashType) -> String {
  let aes = aes::new(aes::AESSupport::HW);
  match hash_type {
    HashType::Cryptonight => hash::hash_alloc_scratchpad(&input, &aes),
    // TODO there are a bunch of warnings generated by unused parts of cryptonightlite.rs
    HashType::CryptonightLite => cryptonightlite::hash_alloc_scratchpad(&input, &aes),
  }
}

#[test]
fn test_hash() {
  let input = byte_string::string_to_u8_array("");
  assert_eq!(cn_hash(input.to_owned(), HashType::Cryptonight),"eb14e8a833fac6fe9a43b57b336789c46ffe93f2868452240720607b14387e11");
  // Test case taken from https://github.com/ExcitableAardvark/node-cryptonight-lite
  assert_eq!(cn_hash(input.to_owned(), HashType::CryptonightLite), "4cec4a947f670ffdd591f89cdb56ba066c31cd093d1d4d7ce15d33704c090611");
  let input2 = byte_string::string_to_u8_array("5468697320697320612074657374");
  assert_eq!(cn_hash(input2, HashType::CryptonightLite), "88e5e684db178c825e4ce3809ccc1cda79cc2adb4406bff93debeaf20a8bebd9");
}

#[derive(Deserialize, Default)]
struct BlockTemplate {
  blockhashing_blob: String,
  blocktemplate_blob: String,
  difficulty: u64,
  height: u64,
  prev_hash: String,
  reserved_offset: u32,
  status: String
}

// TODO eventually this 'allow' will need to go away
#[allow(dead_code)]
struct Job {
  id: String,
  extra_nonce: String,
  height: u64,
  difficulty: u64,
  diff_hex: String,
  template: Arc<Mutex<BlockTemplate>>,
  submissions: ConcHashMap<String, bool>,
}

impl Job {
  fn submit(&self, nonce: &String) -> Result<Value> {
    if nonce.len() != 8 {
      // We expect a hex representing a 32 bit integer.  We don't care so much about validating that
      // it is purely hexadecimal chaaracters, though, since string_to_u8_array will just zero out
      // anything non-hexadecimal.
      return Err(Error::invalid_params("Nonce must be 8 hexadecimal characters"));
    }
    let previous_submission = self.submissions.insert(nonce.to_owned(), true);
    if let Some(_) = previous_submission {
      // TODO we'll probably want some auto banning functionality in place here
      return Err(Error::invalid_params("Nonce already submitted"));
    }
    // TODO check if the block is expired, may want to do away with the template reference and just
    // check against the current template, since anything of a lower height will be expired as long
    // as we only keep one template per height
    let blob = &self.template.lock().unwrap().blockhashing_blob;
    let (a, _) = blob.split_at(78);
    let (_, b) = blob.split_at(86);
    let hash_input = byte_string::string_to_u8_array(&format!("{}{}{}", a, nonce, b));
    println!("Blob to hash: {}\n {} {} {}", blob, a, nonce, b);
    let hash = cn_hash(hash_input, HashType::CryptonightLite);
    let hash_val = byte_string::hex2_u64_le(&hash[48..]);
    // TODO not entirely sure if the line below is correct
    let achieved_difficulty = u64::max_value() / hash_val;
    println!("Hash value: {}, hash: {}, ratio: {}", hash_val, hash, achieved_difficulty);
    if achieved_difficulty >= self.difficulty {
      println!("Valid job submission");
      return Ok(Value::String("Result accepted".to_owned()));
    }
    else {
      println!("Bad job submission");
    }
    Err(Error::internal_error())
  }
}

// TODO eventually this 'allow' will need to go away
#[allow(dead_code)]
struct Miner {
  miner_id: String,
  login: String,
  password: String,
  peer_addr: SocketAddr,
  difficulty: u64,
  jobs: ConcHashMap<String, Job>,
}

impl Miner {
  /// Returns a representation of the miner's current difficulty, in a hex format which is sort of
  /// a quirk of the stratum protocol.
  fn get_target_hex(&self) -> String {
    let min_diff = BigInt::parse_bytes(
      b"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF", 16).unwrap();
    let full_diff = min_diff.div_floor(&BigInt::from(self.difficulty));
    let (_, full_diff_le) = full_diff.to_bytes_le();
    let full_diff_hexes: Vec<String> = full_diff_le[(full_diff_le.len() - 3)..].iter()
      .map(|b| format!("{:02x}", b))
      .collect();
    full_diff_hexes.join("") + "00"
  }

  fn get_job(&self, current_template: &Arc<Mutex<BlockTemplate>>) -> Result<Value> {
    // Notes on the block template:
    // - reserve_size (8) is the amount of bytes to reserve so the pool can throw in an extra nonce
    // - the daemon returns result.reserved_offset, and that many bytes into
    //   result.blocktemplate_blob, we can write our 8 byte extra nonce
    // - the node pools use a global counter, but we might want the counter to be per-miner
    // - it might not even be necessary to use any counters
    //   (and just go with the first 8 bytes of the miner id)
    let arc_template = current_template.clone();
    let template_data = arc_template.lock().unwrap();
    let job_id = &Uuid::new_v4().to_string();
    // TODO remove the bytes dependency if we don't use it
    //let mut buf = BytesMut::with_capacity(128);
    // TODO at least do something to the reserved bytes
    let target_hex = self.get_target_hex();
    let new_job = Job {
      id: job_id.to_owned(),
      extra_nonce: String::new(),
      height: template_data.height,
      difficulty: self.difficulty,
      diff_hex: target_hex.to_owned(),
      template: current_template.clone(),
      submissions: Default::default(),
    };
    self.jobs.insert(job_id.to_owned(), new_job);
    return Ok(json!({
      "job_id": job_id,
      "blob": template_data.blockhashing_blob,
      "target": target_hex,
    }));
    Err(Error::internal_error())
  }
}

#[derive(Default, Clone)]
struct Meta {
  peer_addr: Option<SocketAddr>
}
impl Metadata for Meta {}

struct PoolServer {
  config: ServerConfig,
  // TODO there will need to be expiry here
  miner_connections: ConcHashMap<String, Miner>,
  block_template: Arc<Mutex<BlockTemplate>>,
}

impl PoolServer {
  fn new(server_config: &ServerConfig)-> PoolServer {
    PoolServer {
      config: server_config.clone(),
      miner_connections: Default::default(),
      block_template: Arc::new(Mutex::new(Default::default()))
    }
  }

  fn getminer(&self, params: &Map<String, Value>) -> Option<&Miner> {
    if let Some(&Value::String(ref id)) = params.get("id") {
      if let Some(miner) = self.miner_connections.find(id) {
        let miner: &Miner = miner.get();
        Some(miner)
      } else {
        None
      }
    } else {
      None
    }
  }

  fn getjob(&self, params: Map<String, Value>) -> Result<Value> {
    if let Some(miner) = self.getminer(&params) {
      miner.get_job(&self.block_template)
    }
    else {
      Err(Error::invalid_params("No miner with this ID"))
    }
  }

  fn login(&self, params: Map<String, Value>, meta: Meta) -> Result<Value> {
    if let None = meta.peer_addr {
      return Err(Error::internal_error());
    }
    if let Some(&Value::String(ref login)) = params.get("login") {
      let id = &Uuid::new_v4().to_string();
      // TODO add some validation on the login address
      let miner = Miner {
        miner_id: id.to_owned(),
        login: login.to_owned(),
        // TODO password isn't used, should probably go away
        password: "".to_owned(),
        peer_addr: meta.peer_addr.unwrap(),
        // TODO implement vardiff
        difficulty: self.config.difficulty,
        jobs: Default::default(),
      };
      let response = json!({
        "id": id,
        "job": miner.get_job(&self.block_template)?,
        "status": "OK",
      });
      self.miner_connections.insert(id.to_owned(), miner);
      Ok(response)
    } else {
      Err(Error::invalid_params("Login address required"))
    }
  }

  fn submit(&self, params: Map<String, Value>) -> Result<Value> {
    if let Some(miner) = self.getminer(&params) {
      if let Some(&Value::String(ref job_id)) = params.get("job_id") {
        if let Some(job) = miner.jobs.find(job_id) {
          if let Some(&Value::String(ref nonce)) = params.get("nonce") {
            println!("nonce: {}", nonce);
            return job.get().submit(nonce);
          }
        }
      }
    }
    Err(Error::invalid_params("No miner with this ID"))
  }
}

// TODO this will probably go in another file
fn call_daemon(daemon_url: &str, method: &str, params: Value)
               -> reqwest::Result<Value> {
  let map = json!({
    "jsonrpc": Value::String("2.0".to_owned()),
    "id": Value::String("0".to_owned()),
    "method": Value::String(method.to_owned()),
    "params": params,
  });
  let client = reqwest::Client::new();
  let mut res = client.post(daemon_url)
    .json(&map)
    .send()?;
  res.json()
}

pub fn init(config: Config) {
  // TODO bind the server difficulties to their configs
  let config_ref = Arc::new(config);
  let inner_config_ref = config_ref.clone();
  let servers: Vec<Arc<PoolServer>> = config_ref.ports.iter().map(|server_config| {
    let mut io = MetaIoHandler::with_compatibility(Compatibility::Both);
    //let mut pool_server: PoolServer = PoolServer::new();
    let pool_server: Arc<PoolServer> = Arc::new(PoolServer::new(server_config));
    let login_ref = pool_server.clone();
    io.add_method_with_meta("login", move |params, meta: Meta| {
      // TODO repeating this match isn't pretty
      match params {
        Params::Map(map) => login_ref.login(map, meta),
        _ => Err(Error::invalid_params("Expected a params map")),
      }
    });

    let getjob_ref = pool_server.clone();
    io.add_method("getjob", move |params: Params| {
      // TODO repeating this match isn't pretty
      match params {
        Params::Map(map) => getjob_ref.getjob(map),
        _ => Err(Error::invalid_params("Expected a params map")),
      }
    });

    let submit_ref = pool_server.clone();
    io.add_method("submit", move |params| {
      // TODO repeating this match isn't pretty
      match params {
        Params::Map(map) => submit_ref.submit(map),
        _ => Err(Error::invalid_params("Expected a params map")),
      }
    });

    let _keepalived_ref = pool_server.clone();
    io.add_method("keepalived", |_params| {
      Ok(Value::String("hello".to_owned()))
    });

    let server = ServerBuilder::new(io)
      .session_meta_extractor(|context: &RequestContext| {
        Meta {
          peer_addr: Some(context.peer_addr)
        }
      })
      .start(&SocketAddr::new("127.0.0.1".parse().unwrap(), server_config.port))
      .unwrap();
    thread::spawn(|| server.wait());
    pool_server
  }).collect();

  // TODO make sure we refresh the template after every successful submit
  let thread_config_ref = inner_config_ref.clone();
  let tick = periodic_ms(10000);
  loop {
    let params = json!({
        "wallet_address": thread_config_ref.pool_wallet,
        "reserve_size": 8
      });
    let template = call_daemon(&thread_config_ref.daemon_url, "getblocktemplate", params);
    match template {
      Ok(template) => {
        // TODO verify that checking the height (and not prev_hash) is sufficient
        if let Some(result) = template.get("result") {
          for server in servers.iter() {
            let mut current_template = server.block_template.lock().unwrap();
            let parsed_template: StdResult<BlockTemplate, serde_json::Error> =
              serde_json::from_value(result.clone());
            if let Ok(new_template) = parsed_template {
              if new_template.height > current_template.height {
                println!("New block template of height {}.", new_template.height);
                *current_template = new_template;
              }
            }
          }
        }
      },
      Err(message) => println!("Failed to get new block template: {}", message)
    }
    tick.recv().unwrap();
  }
}

#[test]
fn target_hex_correct() {
  let mut miner = Miner {
    miner_id: String::new(),
    login: String::new(),
    password: String::new(),
    peer_addr: None,
    difficulty: 5000,
    jobs: Default::default(),
  };
  assert_eq!(miner.get_target_hex(), "711b0d00");
  miner.difficulty = 20000;
  assert_eq!(miner.get_target_hex(), "dc460300");
}
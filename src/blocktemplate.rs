use longkeccak::keccak;
use mithril::byte_string;

// TODO all of the tree hashing stuff should go in another file
fn tree_hash_cnt(count: usize) -> usize {
  let mut i = 1;
  while i * 2 < count {
    // TODO this isn't optimal, but maybe we don't care
    i *= 2;
  }
  return i;
}

/// From CNS-3 section 3
/// TODO document this, and really everything else, a bit better
fn get_varint(source: &[u8]) -> (usize, usize) {
  if source[0] < 128 {
    return (source[0] as usize, 1);
  }
  let mut i = 0;
  let mut sum: usize = 0;
  while source[i] > 128 {
    let current_b128_digit = (source[i] - 128) as usize;
    // Shifting by i * 7 is multiplying by 128^i, since 128 is our base.
    sum += current_b128_digit << (i * 7);
    i += 1;
  }
  sum += (source[i] as usize) << (i * 7);
  return (sum, i + 1);
}

#[test]
fn test_get_varint() {
  assert_eq!(get_varint(&[42]), (42, 1));
  assert_eq!(get_varint(&[128 + 1, 42]), (42 * 128 + 1, 2));
  assert_eq!(get_varint(&[128 + 60, 128 + 61, 63]), (63 * 128 * 128 + 61 * 128 + 60, 3));
}

fn parse_block_template(template: Vec<u8>) {
  // TODO maybe remove this, since it likely won't be used
  let mut i = 42;
  // The nonce is from bytes 34-38, and the data we need to compute the mining blob starts at 39.
  let (format, format_len) = get_varint(&template[i..]);
  // println!("format {}", format);
  i += format_len;
  let (version, version_len) = get_varint(&template[i..]);
  // println!("version {}", version);
  i += version_len;
  let (unlock_time, unlock_time_len) = get_varint(&template[i..]);
  // println!("unlock_time {}", unlock_time);
  i += unlock_time_len;
  let (input_num, input_num_len) = get_varint(&template[i..]);
  // println!("input_num {}", input_num);
  i += input_num_len;
  i += 1; // input_type which is one byte
  // TODO height is a good checkpoint
  let (height, height_len) = get_varint(&template[i..]);
  i += height_len;
  // println!("Parsed height: {} from {} to {} ", height, i - height_len, i);
  let (output_num, output_num_len) = get_varint(&template[i..]);
  i += output_num_len;
  // println!("Output num: {}", output_num);
  // note: each key in output is 32 bytes, though is preceded by a varint
  for _ in 0..output_num {
    let (_amount, amount_len) = get_varint(&template[i..]);
    // println!("amount {}", _amount);
    i += amount_len;
    // println!("output_type {}", template[i]);
    i += 1; // 1 byte for output_type
    i += 32; // 32 bytes for the key field;
  }
  // println!("extra_size position: {}", i);
  let extra_size = template[i];
  // println!("extra_size {}", extra_size);
  i += 1 + extra_size as usize;
  let (tx_num, tx_num_len) = get_varint(&template[i..]);
  i += tx_num_len;
  // println!("{} bytes remaining, should be 32 * {}", template.len() - i, tx_num);
  // TODO since the daemon returns the extra_nonce offset, we can add 9 (8 for reserve_size and 1
  // for the tx_size field) to get the start of the miner transactions
  // So much of the above might be unnecessary, but we may still need it to find the start of the
  // miner_tx

  // Note:  need to keep in mind, the last byte of the hashing blob is the number of transactions
  // plus 1
}

#[test]
fn test_parse_block_template() {
  let test_block = BlockTemplate {
    blockhashing_blob: "010094fed5d205e42c97122a7b61341c46881837099891d2b2587a0bde019cbae1688e41bc4\
    d70000000005c8e57bea6b5667f77529149756c249904fb346916f7580c18ea64ec793334e903".to_owned(),
    blocktemplate_blob: "010094fed5d205e42c97122a7b61341c46881837099891d2b2587a0bde019cbae1688e41bc\
    4d700000000001e1cf3701ffa5cf3705fbf3b1e40b02d2961caddbcd6294b41030ecf24fadc4229fc45c75df5def56d\
    c1841236db36380f8cce2840202bdba3913153bbbbd8c40a8b9409fe8944bb9964edd905506b558f8eadf027b858080\
    dd9da41702625f0a1c55924dedd94ae36929cfb99664176ff1d6417abfdc5bfb40daf20b9380a094a58d1d027151b66\
    783aa0ed7d3531dcc35b958945491922222327f9bd57693a18b252a6a80c0caf384a302022c8848debdf1f00e5f6a47\
    f0886e5caf027c8fd7e159277f1aa6c5a3796e49ca2b01bdcff031f0dd952991227c05512204eb76400cd8a06c30458\
    31783cd6fbdb9f50208000000000000000002cde625408d94764cf5244bff45ddb0f8d6d42d02b8c6afb99ae9dff33a\
    7bfcacae531ddf666352c45b25569c8d894ed8a327d9fb3c361ed0e7e0433190fe9fec".to_owned(),
    difficulty: 0,
    height: 0,
    prev_hash: "".to_owned(),
    reserved_offset: 285,
    status: "OK".to_owned(),
  };
  assert_eq!(test_block.blockhashing_blob,
             test_block.hashing_blob_with_nonce("0000000000000000").unwrap());
}

fn concat_and_hash(in1: &[u8], in2: &[u8]) -> Vec<u8> {
  let mut concatted_inputs = in1.to_vec();
  concatted_inputs.extend(in2.iter());
  // TODO not sure if &foo[..] is the best way
  // println!("concatted length {}", concatted_inputs.len());
  return keccak(&concatted_inputs[..])[..32].to_vec();
}

// block ends with miner_tx which is a transaction, and tx_hashes which is a list of 32-byte hashes

#[test]
fn test_tree_hash() {
  // Test case pulled from the monero project's tests directory
  let concat_hash_tests = vec![
    byte_string::string_to_u8_array("21f750d5d938dd4ed1fa4daa4d260beb5b73509de9a9b145624d3f1afb671461"),
    byte_string::string_to_u8_array("b07d768cf1f5f8266b89ecdc150a2ad55ccd76d4c12d3a380b21862809a85af6"),
    byte_string::string_to_u8_array("23269a23ee1b4694b26aa317b5cd4f259925f6b3288a8f60fb871b1ad3ac00cb"),
    byte_string::string_to_u8_array("1e6c55eddfc438e1f3e7b638ea6026cc01495010bafdfd789c47dff282c1af4c"),
    byte_string::string_to_u8_array("6a8f83e5f2fca6940a756ef4faa15c7137082a7c31dffe0b2f5112d126ad4af1"),
    byte_string::string_to_u8_array("d536c0e626cc9d2fe1b72256f5285728558f22a3dbb36e0918bcfc01d4ae7284"),
    byte_string::string_to_u8_array("d0bfb8e90647cdb01c292a53a31ff3fe6f350882f1dae2b09374db45f4d54c67"),
    byte_string::string_to_u8_array("d3b4e0829c4f9f63ad235d8ef838d8fb39546d90d99bbd831aff55dbbb642e2b"),
    byte_string::string_to_u8_array("f529ceccd0479b9f194475c2a15143f0edac762e9bbce810436e765550c69e23"),
    byte_string::string_to_u8_array("4c22276c41d7d7e28c10afc5e144a9ce32aa9c0f28bb4fcf171af7d7404fa5e2"),
    byte_string::string_to_u8_array("8b79dc97bd4147f4df6d38b935bd83fb634414bae9d64a32ab45384fba5b8da5"),
    byte_string::string_to_u8_array("c147d51cd2a8f7f2a9c07b1bddc5b28b74bf0c0f0632ac2fc43d0d306dd1ac14"),
    byte_string::string_to_u8_array("81cabe60a358d6043d4733202d489664a929d6bf76a39828954846beb47a3baa"),
    byte_string::string_to_u8_array("cb35d2065cbe3ad34cf78bf895f6323a6d76fc1256306f58e4baecabd7a77938"),
    byte_string::string_to_u8_array("8c6bf2734897c193d39c343fce49a456f0ef84cf963593c5401a14621cc6ec1b"),
    byte_string::string_to_u8_array("ef01b53735ccb02bc96c5fd454105053e3b016174437ed83b25d2a79a88268f2"),
  ];
  let test_tree_hash: Vec<String> = tree_hash(concat_hash_tests).iter()
    .map(|b| format!("{:02x}", b))
    .collect();
  assert_eq!("2d0ad2566627b50cd45125e89e963433b212b368cd2d91662c44813ba9ec90c2",
             test_tree_hash.join(""));
}

// https://lab.getmonero.org/pubs/MRL-0002.pdf
fn tree_hash(hashes: Vec<Vec<u8>>) -> Vec<u8> {
  // TODO get rid of the commented out print statements and leftover C lines
  let count = hashes.len();
  //assert(count > 0);
  if count == 1 {
    return hashes[0].to_vec();
  } else if count == 2 {
    return concat_and_hash(&hashes[0], &hashes[1]);
  } else {
    let mut cnt = tree_hash_cnt(count);
    //let max_size_t = (size_t) - 1; // max allowed value of size_t
    //assert(cnt < max_size_t / 2); // reasonable size to avoid any overflows. /2 is extra; Anyway should be limited much stronger by logical code
    // as we have sane limits on transactions counts in blockchain rules

    // char(*ints)[HASH_SIZE];
    // let ints_size = cnt * HASH_SIZE;
    // ints = alloca(ints_size);
    let mut ints: Vec<Vec<u8>> = Vec::new();
    let slice_point = 2 * cnt - count;
    // memcpy(ints, hashes, (2 * cnt - count) * HASH_SIZE);
    for i in 0..slice_point {
      ints.push(hashes[i].clone())
    }
    // memset(ints, 0, ints_size);  // allocate, and zero out as extra protection for using uninitialized mem
    for i in slice_point..count {
      ints.push(vec![0]);
    }
    /// example, count = 42, then cnt = 32
    /// i,j start off at (2 * 32) - 42, or 22
    /// each iter i = 22 + (2 * (j - 22))
    /// last iter, j = 31
    /// i = 40
    /// so i in [22..40] step 2
    /// j in [22..32] step 1
    /// more generally, i in [(2*cnt - count)..(count)] step 2
    /// and, j in [(2*cnt - count)..(cnt)] step 1
    let mut i = slice_point;
    for j in slice_point..cnt {
      // actually, we want hashes[i] concatted with hashes[i]+1
      // println!("slice point: {}, i: {}, cnt: {}", slice_point, i, cnt);
      // println!("lengths: {}, {}", hashes[i].len(), hashes[i + 1].len());
      ints[j] = concat_and_hash(&hashes[i], &hashes[i + 1]);
      i += 2;
      //cn_fast_hash(hashes[i], 64, ints[j]);
    }
    //assert(i == count);
    // println!("sanity check, {} vs {}", i, count);

    while cnt > 2 {
      cnt /= 2;
      // cnt >>= 1;;
      let mut ii = 0;
      for jj in 0..cnt {
        // println!("slice point: {}, ii: {}, cnt: {}", slice_point, ii, cnt);
        // println!("lengths: {}, {}", ints[ii].len(), ints[ii + 1].len());
        ints[jj] = concat_and_hash(&ints[ii], &ints[ii + 1]);
        ii += 2;
        //cn_fast_hash(ints[i], 64, ints[j]);
      }
    }

    return concat_and_hash(&ints[0], &ints[1]).to_vec();
  }
}

#[derive(Deserialize, Default)]
pub struct BlockTemplate {
  // TODO eventually most of this stuff can be private
  pub blockhashing_blob: String,
  pub blocktemplate_blob: String,
  pub difficulty: u64,
  pub height: u64,
  pub prev_hash: String,
  pub reserved_offset: u32,
  pub status: String
}

impl BlockTemplate {
  pub fn hashing_blob_with_nonce(&self, nonce: &str) -> Option<String> {
    // TODO document this slicing stuff
    let miner_tx = format!(
      "{}{}",
      &self.blocktemplate_blob[86..((self.reserved_offset * 2 - 2) as usize)],
      nonce
    );
    let miner_tx_hash = keccak(&byte_string::string_to_u8_array(&miner_tx))[..32].to_vec();
    let hex_digits_left = (self.blocktemplate_blob.len() - miner_tx.len()) - 86;
    if (hex_digits_left - 2) % 64 != 0 {
      println!("{}", hex_digits_left);
      return None;
    }
    let mut tx_hashes = Vec::new();
    tx_hashes.push(miner_tx_hash);
    for tx_index in 0..(hex_digits_left / 64) {
      // TODO make these numbers less magic, maybe just increment an index for readability
      // TODO the "2" here assumes 1 byte for transaction count, that needs to be a varint as well
      let start = miner_tx.len() + 86 + 2 + 64 * tx_index;
      tx_hashes.push(byte_string::string_to_u8_array(&self.blocktemplate_blob[start..(start + 64)]));
    }
    let num_hashes = tx_hashes.len();
    let root_hash: Vec<String> = tree_hash(tx_hashes).iter()
      .map(|b| format!("{:02x}", b))
      .collect();
    // TODO the count needs to be a varint since we can have more than 255 transactions in a block
    return Some(
      format!("{}{}{:02x}", &self.blockhashing_blob[..86], &root_hash.join(""), num_hashes)
    );
  }
}

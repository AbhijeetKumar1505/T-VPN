//! Minimal extraction from AWS EC2 Query API XML responses. EC2 responses are
//! machine-generated, attribute-free on the elements we read, and well formed,
//! so exact tag matching is enough.

/// Text inside the first `<tag>…</tag>` (exact tag name), entity-decoded.
pub fn first(xml: &str, tag: &str) -> Option<String> {
    first_raw(xml, tag).map(decode)
}

/// The raw inner XML of the first `<tag>…</tag>`, for scoping further lookups.
pub fn first_raw<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + find_matching_close(&xml[start..], &open, &close)?;
    Some(&xml[start..end])
}

/// Offset of the `close` tag that matches an already-consumed `open`, honoring nesting.
fn find_matching_close(s: &str, open: &str, close: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut pos = 0;
    loop {
        let next_close = s[pos..].find(close)? + pos;
        match s[pos..].find(open).map(|o| o + pos) {
            Some(next_open) if next_open < next_close => {
                depth += 1;
                pos = next_open + open.len();
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some(next_close);
                }
                pos = next_close + close.len();
            }
        }
    }
}

/// Split `xml` into chunks, each starting at an occurrence of `<tag>`. Used to
/// iterate over instances by their `<instanceId>` element.
pub fn chunks_starting_at<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let starts: Vec<usize> = xml.match_indices(&open).map(|(i, _)| i).collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let end = starts.get(n + 1).copied().unwrap_or(xml.len());
            &xml[s..end]
        })
        .collect()
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIBE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DescribeInstancesResponse xmlns="http://ec2.amazonaws.com/doc/2016-11-15/">
  <requestId>abc</requestId>
  <reservationSet><item><reservationId>r-1</reservationId><instancesSet>
    <item><instanceId>i-0aaa</instanceId><imageId>ami-1</imageId>
      <instanceState><code>16</code><name>running</name></instanceState>
      <privateIpAddress>172.31.0.5</privateIpAddress><ipAddress>13.233.1.2</ipAddress>
      <launchTime>2026-11-22T10:00:00.000Z</launchTime>
      <tagSet><item><key>Name</key><value>tvpn &amp; co</value></item></tagSet>
    </item>
    <item><instanceId>i-0bbb</instanceId>
      <instanceState><code>0</code><name>pending</name></instanceState>
    </item>
  </instancesSet></item></reservationSet>
</DescribeInstancesResponse>"#;

    #[test]
    fn reads_nested_values() {
        assert_eq!(first(DESCRIBE, "instanceId").as_deref(), Some("i-0aaa"));
        let state = first_raw(DESCRIBE, "instanceState").unwrap();
        assert_eq!(first(state, "name").as_deref(), Some("running"));
        assert_eq!(first(DESCRIBE, "ipAddress").as_deref(), Some("13.233.1.2"));
        assert_eq!(first(DESCRIBE, "value").as_deref(), Some("tvpn & co"));
        assert_eq!(first(DESCRIBE, "missing"), None);
    }

    #[test]
    fn nested_same_name_tags() {
        let set = first_raw(DESCRIBE, "instancesSet").unwrap();
        assert!(set.contains("i-0bbb"), "matching close must skip nested <item>s");
        let outer = first_raw(DESCRIBE, "item").unwrap();
        assert!(outer.contains("i-0bbb"));
    }

    #[test]
    fn iterates_instances() {
        let chunks = chunks_starting_at(DESCRIBE, "instanceId");
        assert_eq!(chunks.len(), 2);
        assert_eq!(first(chunks[0], "ipAddress").as_deref(), Some("13.233.1.2"));
        assert_eq!(first(chunks[1], "ipAddress"), None);
        assert_eq!(first(first_raw(chunks[1], "instanceState").unwrap(), "name").as_deref(), Some("pending"));
    }

    #[test]
    fn errors() {
        let err = "<Response><Errors><Error><Code>UnauthorizedOperation</Code><Message>You are not authorized</Message></Error></Errors></Response>";
        assert_eq!(first(err, "Code").as_deref(), Some("UnauthorizedOperation"));
        assert_eq!(first(err, "Message").as_deref(), Some("You are not authorized"));
    }
}

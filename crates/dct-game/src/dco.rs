//! 连本机 dco 的客户端在 `dct-dco`；这里只把它接到三消要的 `Dco` 上。
use crate::board::GridRead;
use crate::play::{Dco, DcoError, Profile, Seen};
use crate::screen::Element;
pub use dct_dco::{DcoClient, IO_TIMEOUT};
use serde_json::json;

impl Dco for DcoClient {
    fn read_grid(&mut self, p: &Profile) -> Result<GridRead, DcoError> {
        let mut args = json!({
            "window": p.window,
            "region": { "x": p.region[0], "y": p.region[1], "w": p.region[2], "h": p.region[3] },
            "rows": p.rows, "cols": p.cols,
        });
        if let (Some(extra), Some(obj)) = (p.extra.as_object(), args.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        let body = self.call("read_grid", args)?;
        serde_json::from_value(body).map_err(|e| DcoError { code: "dco_error".into(), message: format!("dco 的棋盘回复读不懂：{e}") })
    }

    fn swipe(&mut self, p: &Profile, from: (f64, f64), to: (f64, f64)) -> Result<(), DcoError> {
        self.call("swipe", json!({
            "window": p.window,
            "from": { "x": from.0, "y": from.1 }, "to": { "x": to.0, "y": to.1 },
        }))
        .map(|_| ())
    }

    fn see_text(&mut self, p: &Profile) -> Result<Seen, DcoError> {
        let body = self.call("see", json!({ "window": p.window, "source": "ocr" }))?;
        let elements = body["elements"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| Some(Element { id: e["id"].as_str()?.to_string(), text: e["text"].as_str().unwrap_or("").to_string() }))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Seen {
            snapshot_id: body["snapshot_id"].as_str().unwrap_or("").to_string(),
            observation_id: body["observation_id"].as_str().map(String::from),
            elements,
        })
    }

    fn show_status(&mut self, state: &str) {
        if self.no_show_status {
            return;
        }
        // 结果不重要。旧 dco 没这个工具、或者超时，都别再试：超时一次就是 10 秒，每步一次游戏就拖死了。
        if let Err(e) = self.call("show_status", json!({ "state": state })) {
            if e.code == "dco_too_old" || e.code == "dco_timeout" {
                self.no_show_status = true;
            }
        }
    }

    fn tap(&mut self, snapshot_id: &str, element_id: &str) -> Result<(), DcoError> {
        self.call("tap", json!({ "snapshot_id": snapshot_id, "element_id": element_id })).map(|_| ())
    }
}

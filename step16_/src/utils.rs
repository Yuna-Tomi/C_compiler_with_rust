// Errorの報告をする関数(ほぼeprint!のラッパ)
// これを使う際は使う側でuseが必要なことに注意
#[macro_export]
macro_rules! exit_eprint {
	// 引数なし
	() => {
		std::process::exit(1);
	};
	// 文字列リテラルのみの引数
	($fmt:expr) => {
		eprint!($fmt);
		std::process::exit(1);
	};

	// 第二引数以降がある場合 
	($fmt:expr, $($arg:tt)*) =>{
		eprint!($fmt, $($arg)*);
		std::process::exit(1);
	};

}

// eprintln!のラッパ
#[macro_export]
macro_rules! exit_eprintln {
	// 引数なし
	() => {
		eprint!("\n");
		std::process::exit(1);
	};
	// 文字列リテラルのみの引数
	($fmt:expr) => {
		eprint!(concat!($fmt, "\n"));
		std::process::exit(1);
	};

	// 第二引数以降がある場合 
	($fmt:expr, $($arg:tt)*) =>(
		eprint!(concat!($fmt, "\n"),$($arg)*);
		std::process::exit(1);
	);

}

// エラー位置を報告するバージョンを作りたかったが、今の実装でやるのが難しそうなので保留
#[macro_export]
macro_rules!  error_at{
	($fmt: expr, $num: expr, $($arg:tt)*) => {
		let space = " ".to_string().repeat(*$num);
		eprint!(concat!(space, $fmt, "\n"),$($arg)*);
		std::process::exit(1);
	};
}
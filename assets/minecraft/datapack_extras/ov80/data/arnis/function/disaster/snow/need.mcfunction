$execute if score #c$(x)_$(z) arnis.ds.snow = #lv_snow arnis.ds run return 0
$execute if score #lv_snow arnis.ds matches 0 unless score #c$(x)_$(z) arnis.ds.snow matches 1.. run return 0
function arnis:disaster/snow/try with storage arnis:ds scan

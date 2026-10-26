# Kept short, as it runs for every cell near every player and a macro is parsed each time.
$execute unless data storage arnis:ds $(map).cells."$(x)_$(z)" run return 0
$execute if score #c$(x)_$(z) arnis.ds.$(map) = #lv_$(map) arnis.ds run return 0
$execute if score #lv_$(map) arnis.ds matches 0 unless score #c$(x)_$(z) arnis.ds.$(map) matches 1.. run return 0
function arnis:disaster/water/try with storage arnis:ds scan
